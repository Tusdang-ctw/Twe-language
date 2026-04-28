//! AST → bytecode compiler.
//!
//! Phase-3 session 6: literal, unary, binary, comparison expressions.
//! Phase-3 session 8: locals, `let`/`var`, plain-name assignment,
//! `if`/`elif`/`else`, `while`, `break`, `continue`, short-circuit
//! `and` / `or`, top-level `print(<expr>)` lowered to `OP_PRINT`.
//! Phase-3 session 9: top-level `let`/`var` lower to globals,
//! function declarations + calls + returns, and the compiler
//! grows a frame stack so nested function compilation is natural.
//! Phase-3 session 10: heap-type literals (tuple, list, range),
//! indexing, string interpolation, the `in` / `not in` operator,
//! tuple `.x/.y/.z` and list `.length` field access, list/range
//! built-in methods via `OP_INVOKE`, and `for x in iter:` over
//! ranges, lists, and tuples.
//! Phase-3 session 11: `entity` / `item` / `modifier` / `inventory`
//! declarative blocks with field defaults and methods, instance
//! field get/set including `self.x = ...`, instance method
//! dispatch with `self` at slot 0, and module-builtin access
//! (`math.min`, `key.right`, etc.) via `OP_GET_FIELD` and
//! `OP_INVOKE` on `Value::Object`.
//!
//! Scenes, `particles`, states, every-clocks, `on update(dt):`,
//! `spawn` / `despawn` / `transition`, and the play-loop
//! integration come in session 12. Until then, those produce
//! `CompileError`s that name the session where the feature lands.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::{AssignOp, AssignTarget, BinOp, DeclKind, DeclMember, Expr, Program, Stmt, StateMember, UnOp};
use crate::bytecode::{BcClassDef, BcFunction, BcStateDef, Chunk, OpCode};
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
///
/// Used by the VM tests for compact "evaluate this expression"
/// flows. The synthetic outer frame is a script frame, so global
/// lookups inside the expression resolve via `OP_GET_GLOBAL`.
pub fn compile_expr(expr: &Expr) -> Result<Chunk, CompileError> {
    let mut c = Compiler::new();
    c.emit_expr(expr)?;
    let line = expr.line();
    c.frame_mut().chunk.write_op(OpCode::Return, line);
    Ok(c.frames.pop().expect("script frame").chunk)
}

/// Compile a whole program into a chunk that ends in `OP_RETURN`.
/// Top-level `let` / `var` declarations become globals; function
/// declarations register a `Value::BcFunction` constant and bind
/// it as a global. The script's chunk leaves nothing on the stack
/// at `OP_RETURN`, so the VM returns `Nil`.
pub fn compile_program(program: &Program) -> Result<Chunk, CompileError> {
    let mut c = Compiler::new();
    for stmt in &program.stmts {
        c.emit_stmt(stmt)?;
    }
    let line = program.stmts.last().map(stmt_line).unwrap_or(1);
    c.frame_mut().chunk.write_op(OpCode::Return, line);
    Ok(c.frames.pop().expect("script frame").chunk)
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    /// The outermost frame. Top-level `let`/`var` become globals.
    /// `return` is illegal here.
    Script,
    /// A function body. Top-level locals (scope_depth 0 inside the
    /// function) live on the stack; `return` is legal.
    Function,
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

/// Per-function compilation state. Each frame owns its own chunk,
/// locals table, scope depth, and live loop frames. A frame stack
/// lets nested function declarations compile naturally: push a
/// frame, compile the body, pop the frame, lift the chunk into a
/// `BcFunction` constant in the outer frame.
struct Frame {
    chunk: Chunk,
    locals: Vec<Local>,
    scope_depth: i32,
    loops: Vec<LoopFrame>,
    kind: FrameKind,
    /// True if this is a method body. Slot 0 is the receiver
    /// (`self`); `Expr::SelfRef` reads from there.
    is_method: bool,
    /// Inside a method or state body, the field names of the
    /// enclosing class. A bare name that isn't a local routes to
    /// `self.name` if it's in this set (matching the tree-walker's
    /// scope chain: locals → self.fields → globals).
    class_fields: Option<Rc<HashSet<String>>>,
    /// The function's name (for diagnostics) and arity. Only used
    /// when the frame closes — the resulting `BcFunction` carries
    /// these fields.
    name: String,
    arity: u8,
}

impl Frame {
    fn new(kind: FrameKind, name: impl Into<String>, arity: u8) -> Self {
        Frame::with_method(kind, name, arity, false, None)
    }

    fn with_method(
        kind: FrameKind,
        name: impl Into<String>,
        arity: u8,
        is_method: bool,
        class_fields: Option<Rc<HashSet<String>>>,
    ) -> Self {
        // Slot 0 of every frame is reserved for the function value
        // itself (per Crafting Interpreters §24.4.2). For methods
        // it's the receiver. Either way it's nameless to the user.
        let mut locals = Vec::with_capacity(8);
        locals.push(Local { name: String::new(), depth: 0 });
        Self {
            chunk: Chunk::new(),
            locals,
            scope_depth: 0,
            loops: Vec::new(),
            kind,
            is_method,
            class_fields,
            name: name.into(),
            arity,
        }
    }
}

struct Compiler {
    frames: Vec<Frame>,
}

impl Compiler {
    fn new() -> Self {
        Self {
            frames: vec![Frame::new(FrameKind::Script, "<script>", 0)],
        }
    }

    fn frame(&self) -> &Frame {
        self.frames.last().expect("at least one frame")
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("at least one frame")
    }

    // --- statement emission ---

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        match stmt {
            Stmt::Let { name, value, line, col } => {
                self.emit_let(name, value, *line, *col)?;
            }
            Stmt::Assign { target, op, value, line, col } => {
                self.emit_assign(target, *op, value, *line, *col)?;
            }
            Stmt::Expr(e) => {
                // Top-level call to the bare `print` builtin lowers to
                // a single OP_PRINT — keeps Phase-3 programs printable
                // before the full builtin pipeline reaches bytecode.
                if let Expr::Call { callee, args, kwargs, line, col } = e {
                    if kwargs.is_empty() {
                        if let Expr::Ident { name, .. } = callee.as_ref() {
                            if name == "print" && args.len() == 1 {
                                self.emit_expr(&args[0])?;
                                self.frame_mut().chunk.write_op(OpCode::Print, *line);
                                return Ok(());
                            }
                            if name == "print" && args.is_empty() {
                                let idx = self
                                    .frame_mut()
                                    .chunk
                                    .add_constant(Value::Str(Rc::new(String::new())));
                                self.frame_mut().chunk.write_op(OpCode::Constant, *line);
                                self.frame_mut().chunk.write_byte(idx, *line);
                                self.frame_mut().chunk.write_op(OpCode::Print, *line);
                                return Ok(());
                            }
                            if name == "print" {
                                return Err(CompileError {
                                    line: *line,
                                    col: *col,
                                    message:
                                        "bytecode `print(...)` shorthand takes 0 or 1 args; \
                                         multi-arg print lands with full builtin dispatch in session 10"
                                            .to_string(),
                                });
                            }
                        }
                    }
                }
                self.emit_expr(e)?;
                self.frame_mut().chunk.write_op(OpCode::Pop, e.line());
            }
            Stmt::If { cond, then_body, elifs, else_body, line, .. } => {
                self.emit_if(cond, then_body, elifs, else_body.as_deref(), *line)?;
            }
            Stmt::While { cond, body, line, .. } => {
                self.emit_while(cond, body, *line)?;
            }
            Stmt::Break { line, col } => {
                if self.frame().loops.is_empty() {
                    return Err(CompileError {
                        line: *line,
                        col: *col,
                        message: "`break` outside of a loop".to_string(),
                    });
                }
                let frame = self.frame();
                let pop_count = frame.locals.len() - frame.loops.last().unwrap().locals_at_entry;
                for _ in 0..pop_count {
                    self.frame_mut().chunk.write_op(OpCode::Pop, *line);
                }
                let jmp = self.emit_jump(OpCode::Jump, *line);
                self.frame_mut().loops.last_mut().unwrap().breaks.push(jmp);
            }
            Stmt::Continue { line, col } => {
                if self.frame().loops.is_empty() {
                    return Err(CompileError {
                        line: *line,
                        col: *col,
                        message: "`continue` outside of a loop".to_string(),
                    });
                }
                let frame = self.frame();
                let loop_frame = frame.loops.last().unwrap();
                let pop_count = frame.locals.len() - loop_frame.locals_at_entry;
                let loop_start = loop_frame.loop_start;
                for _ in 0..pop_count {
                    self.frame_mut().chunk.write_op(OpCode::Pop, *line);
                }
                self.emit_loop(loop_start, *line)?;
            }
            Stmt::Return { value, line, col } => {
                if self.frame().kind == FrameKind::Script {
                    return Err(CompileError {
                        line: *line,
                        col: *col,
                        message: "`return` is only valid inside a function body".to_string(),
                    });
                }
                match value {
                    Some(e) => self.emit_expr(e)?,
                    None => self.frame_mut().chunk.write_op(OpCode::Nil, *line),
                }
                self.frame_mut().chunk.write_op(OpCode::Return, *line);
            }
            Stmt::FunctionDecl { name, params, body, line, col } => {
                self.emit_function_decl(name, params, body, *line, *col)?;
            }
            Stmt::Decl { kind, name, parent, members, line, col } => {
                self.emit_decl(*kind, name, parent.as_deref(), members, *line, *col)?;
            }
            Stmt::OnUpdate { param, body, line, col } => {
                if !self.is_global_scope() {
                    return Err(CompileError {
                        line: *line,
                        col: *col,
                        message: "top-level `on update(dt):` is only valid at the script root"
                            .to_string(),
                    });
                }
                // Compile the handler as a non-method function with
                // `dt` at slot 1; then OP_SET_ON_UPDATE binds it on
                // the VM as the per-tick handler.
                let func = self.compile_top_level_on_update(param, body, *line, *col)?;
                let func_idx = self
                    .frame_mut()
                    .chunk
                    .add_constant(Value::BcFunction(Rc::new(func)));
                self.frame_mut().chunk.write_op(OpCode::SetOnUpdate, *line);
                self.frame_mut().chunk.write_byte(func_idx, *line);
            }
            Stmt::For { var, iter, body, line, .. } => {
                self.emit_for(var, iter, body, *line)?;
            }
            Stmt::Transition { target, line, .. } => {
                let name_idx = self
                    .frame_mut()
                    .chunk
                    .add_constant(Value::Str(Rc::new(target.clone())));
                self.frame_mut().chunk.write_op(OpCode::Transition, *line);
                self.frame_mut().chunk.write_byte(name_idx, *line);
            }
            Stmt::Spawn { class, at, line, .. } => {
                // `spawn ClassName at <expr>` — emit at-value (if any),
                // then the class as a global lookup, then OP_SPAWN with
                // a flag indicating whether at is on the stack.
                let with_at = at.is_some();
                if let Some(at_expr) = at {
                    self.emit_expr(at_expr)?;
                }
                let name_idx = self
                    .frame_mut()
                    .chunk
                    .add_constant(Value::Str(Rc::new(class.clone())));
                self.frame_mut().chunk.write_op(OpCode::GetGlobal, *line);
                self.frame_mut().chunk.write_byte(name_idx, *line);
                self.frame_mut().chunk.write_op(OpCode::Spawn, *line);
                self.frame_mut().chunk.write_byte(if with_at { 1 } else { 0 }, *line);
            }
            Stmt::Despawn { target, line, .. } => {
                self.emit_expr(target)?;
                self.frame_mut().chunk.write_op(OpCode::Despawn, *line);
            }
        }
        Ok(())
    }

    fn emit_let(
        &mut self,
        name: &str,
        value: &Expr,
        line: u32,
        col: u32,
    ) -> Result<(), CompileError> {
        if self.is_global_scope() {
            // Global path: evaluate RHS, then bind by name.
            self.emit_expr(value)?;
            let name_idx = self
                .frame_mut()
                .chunk
                .add_constant(Value::Str(Rc::new(name.to_string())));
            self.frame_mut().chunk.write_op(OpCode::DefineGlobal, line);
            self.frame_mut().chunk.write_byte(name_idx, line);
        } else {
            // Local path: evaluate RHS (it stays on the stack as the
            // local's slot), then register the name.
            self.emit_expr(value)?;
            self.declare_local(name, line, col)?;
            self.mark_initialised();
        }
        Ok(())
    }

    fn emit_function_decl(
        &mut self,
        name: &str,
        params: &[String],
        body: &[Stmt],
        line: u32,
        col: u32,
    ) -> Result<(), CompileError> {
        if !self.is_global_scope() {
            return Err(CompileError {
                line,
                col,
                message: "function declarations are only supported at top level in v0.1 \
                          (nested closures arrive when the upvalue pass lands)"
                    .to_string(),
            });
        }
        if params.len() > u8::MAX as usize {
            return Err(CompileError {
                line,
                col,
                message: format!(
                    "function `{name}` has too many parameters (max {})",
                    u8::MAX
                ),
            });
        }
        let arity = params.len() as u8;

        // Open the function frame and pre-declare each parameter as
        // an initialised local. Slot 0 (the function value itself)
        // is already reserved by `Frame::new`.
        self.frames.push(Frame::new(FrameKind::Function, name, arity));
        for param in params {
            self.declare_local(param, line, col)?;
            self.mark_initialised();
        }

        // Compile the body.
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        // Implicit `return nil` if the body didn't return on its own
        // (Crafting Interpreters §24.5.1: every chunk ends in OP_RETURN).
        let last_line = body.last().map(stmt_line).unwrap_or(line);
        self.frame_mut().chunk.write_op(OpCode::Nil, last_line);
        self.frame_mut().chunk.write_op(OpCode::Return, last_line);

        let function_frame = self.frames.pop().expect("function frame we just pushed");
        let function = BcFunction::new(function_frame.name, function_frame.arity, function_frame.chunk);
        let func_idx = self
            .frame_mut()
            .chunk
            .add_constant(Value::BcFunction(Rc::new(function)));
        self.frame_mut().chunk.write_op(OpCode::Constant, line);
        self.frame_mut().chunk.write_byte(func_idx, line);

        // Bind the function to its name as a global. (When closures
        // arrive, this branches: locals get an OP_DEFINE_LOCAL slot.)
        let name_idx = self
            .frame_mut()
            .chunk
            .add_constant(Value::Str(Rc::new(name.to_string())));
        self.frame_mut().chunk.write_op(OpCode::DefineGlobal, line);
        self.frame_mut().chunk.write_byte(name_idx, line);
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
        let loop_start = self.frame().chunk.code.len();
        let locals_at_entry = self.frame().locals.len();
        self.frame_mut().loops.push(LoopFrame {
            loop_start,
            locals_at_entry,
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
        let frame = self.frame_mut().loops.pop().expect("loop frame missing");
        for b in frame.breaks {
            self.patch_jump(b)?;
        }
        Ok(())
    }

    /// Emit a `for var in iter:` loop. The compiled shape:
    /// ```text
    ///   <eval iter>            ; pushes iter, declared as hidden local
    ///   OP_CONSTANT 0          ; push counter, declared as hidden local
    /// loop_start:
    ///   OP_FOR_NEXT base, exit ; on iter: increments counter, pushes
    ///                          ;   element (becomes user var local)
    ///                          ; on exhaust: jumps to exit
    ///   <body>                 ; user var visible inside body's scope
    ///   ;; end_scope pops the user var (the elem FOR_NEXT pushed)
    ///   OP_LOOP loop_start
    /// exit:
    ///   OP_POP                 ; counter
    ///   OP_POP                 ; iter
    /// ```
    /// The hidden iter and counter occupy adjacent slots so OP_FOR_NEXT
    /// only needs the base slot. Their nameless locals don't shadow
    /// any user names because `resolve_local` skips empty-name entries
    /// implicitly (no source identifier can have empty text).
    fn emit_for(
        &mut self,
        var: &str,
        iter: &Expr,
        body: &[Stmt],
        line: u32,
    ) -> Result<(), CompileError> {
        let var_col = 0;
        // Push iter onto the stack, register a hidden local for it.
        // Names start with a space so they can't collide with any real
        // identifier (the lexer never produces names starting with whitespace).
        let iter_name = format!(" __for_iter_{}", self.frame().chunk.code.len());
        let counter_name = format!(" __for_counter_{}", self.frame().chunk.code.len());
        self.emit_expr(iter)?;
        self.declare_local(&iter_name, line, var_col)?;
        self.mark_initialised();
        let base_slot = (self.frame().locals.len() - 1) as u8;
        // Push counter (Int 0), register hidden local.
        let zero_idx = self.frame_mut().chunk.add_constant(Value::Int(0));
        self.frame_mut().chunk.write_op(OpCode::Constant, line);
        self.frame_mut().chunk.write_byte(zero_idx, line);
        self.declare_local(&counter_name, line, var_col)?;
        self.mark_initialised();

        // Loop frame: body locals start above the counter; break /
        // continue pop down to here. Note: break additionally jumps
        // past the post-loop OP_POP×2, so it doesn't double-pop the
        // hidden iter/counter — see the breaks-patch step below.
        let loop_start = self.frame().chunk.code.len();
        let body_locals = self.frame().locals.len();
        self.frame_mut().loops.push(LoopFrame {
            loop_start,
            locals_at_entry: body_locals,
            breaks: Vec::new(),
        });

        // Emit OP_FOR_NEXT with a placeholder exit jump.
        self.frame_mut().chunk.write_op(OpCode::ForNext, line);
        self.frame_mut().chunk.write_byte(base_slot, line);
        self.frame_mut().chunk.write_byte(0xFF, line);
        self.frame_mut().chunk.write_byte(0xFF, line);
        let exit_jump_offset = self.frame().chunk.code.len() - 2;

        // Body scope. FOR_NEXT pushed the element; bind it to `var`
        // as a body-scope local so end_scope's OP_POP retires it.
        self.begin_scope();
        self.declare_local(var, line, var_col)?;
        self.mark_initialised();
        for s in body {
            self.emit_stmt(s)?;
        }
        self.end_scope(line);
        self.emit_loop(loop_start, line)?;

        // Patch FOR_NEXT's exit and all break jumps to land here, then
        // emit the OP_POP × 2 that retires the hidden iter + counter.
        self.patch_jump(exit_jump_offset)?;
        let frame = self.frame_mut().loops.pop().expect("loop frame missing");
        for b in frame.breaks {
            self.patch_jump(b)?;
        }
        self.frame_mut().chunk.write_op(OpCode::Pop, line); // counter
        self.frame_mut().chunk.write_op(OpCode::Pop, line); // iter
        // Drop the hidden locals from the compile-time table so later
        // declarations don't think the slots are still in use.
        self.frame_mut().locals.pop();
        self.frame_mut().locals.pop();
        Ok(())
    }

    fn emit_assign(
        &mut self,
        target: &AssignTarget,
        op: AssignOp,
        value: &Expr,
        line: u32,
        _col: u32,
    ) -> Result<(), CompileError> {
        match target {
            AssignTarget::Name(name) => {
                if let Some(slot) = self.resolve_local(name) {
                    if matches!(op, AssignOp::Set) {
                        self.emit_expr(value)?;
                    } else {
                        // Compound: load current local, evaluate RHS, apply op.
                        self.frame_mut().chunk.write_op(OpCode::GetLocal, line);
                        self.frame_mut().chunk.write_byte(slot, line);
                        self.emit_expr(value)?;
                        let arith = arith_for_compound(op);
                        self.frame_mut().chunk.write_op(arith, line);
                    }
                    self.frame_mut().chunk.write_op(OpCode::SetLocal, line);
                    self.frame_mut().chunk.write_byte(slot, line);
                    self.frame_mut().chunk.write_op(OpCode::Pop, line);
                } else if self.is_self_field(name) {
                    // Bare-name assignment routes through self.field
                    // when the class declares it. Compile shape mirrors
                    // AssignTarget::Field but with the receiver pulled
                    // from slot 0 instead of an explicit object expr.
                    let name_idx = self
                        .frame_mut()
                        .chunk
                        .add_constant(Value::Str(Rc::new(name.clone())));
                    if matches!(op, AssignOp::Set) {
                        // Stack: [self, value]
                        self.frame_mut().chunk.write_op(OpCode::GetLocal, line);
                        self.frame_mut().chunk.write_byte(0, line);
                        self.emit_expr(value)?;
                    } else {
                        // Stack: [self, current, value], then arith → [self, new]
                        self.frame_mut().chunk.write_op(OpCode::GetLocal, line);
                        self.frame_mut().chunk.write_byte(0, line);
                        self.frame_mut().chunk.write_op(OpCode::GetLocal, line);
                        self.frame_mut().chunk.write_byte(0, line);
                        self.frame_mut().chunk.write_op(OpCode::GetField, line);
                        self.frame_mut().chunk.write_byte(name_idx, line);
                        self.emit_expr(value)?;
                        let arith = arith_for_compound(op);
                        self.frame_mut().chunk.write_op(arith, line);
                    }
                    self.frame_mut().chunk.write_op(OpCode::SetField, line);
                    self.frame_mut().chunk.write_byte(name_idx, line);
                    self.frame_mut().chunk.write_op(OpCode::Pop, line);
                } else {
                    // Global path. Add the name string to the constant
                    // pool once and reuse the index for the get + set.
                    let name_idx = self
                        .frame_mut()
                        .chunk
                        .add_constant(Value::Str(Rc::new(name.clone())));
                    if matches!(op, AssignOp::Set) {
                        self.emit_expr(value)?;
                    } else {
                        self.frame_mut().chunk.write_op(OpCode::GetGlobal, line);
                        self.frame_mut().chunk.write_byte(name_idx, line);
                        self.emit_expr(value)?;
                        let arith = arith_for_compound(op);
                        self.frame_mut().chunk.write_op(arith, line);
                    }
                    self.frame_mut().chunk.write_op(OpCode::SetGlobal, line);
                    self.frame_mut().chunk.write_byte(name_idx, line);
                    self.frame_mut().chunk.write_op(OpCode::Pop, line);
                }
            }
            AssignTarget::Field { object, name } => {
                // For `recv.name = value`, OP_SET_FIELD wants
                // [..., recv, value] on the stack. Compound forms
                // (`recv.name += value`) load via OP_GET_FIELD first,
                // arith, then SET. Either way we end with OP_POP to
                // discard the value SetField left on top.
                if matches!(op, AssignOp::Set) {
                    self.emit_expr(object)?;
                    self.emit_expr(value)?;
                } else {
                    // Compound: emit object twice (once to load
                    // current value, once as recv for the SET).
                    self.emit_expr(object)?;
                    self.emit_expr(object)?;
                    let load_name = self
                        .frame_mut()
                        .chunk
                        .add_constant(Value::Str(Rc::new(name.clone())));
                    self.frame_mut().chunk.write_op(OpCode::GetField, line);
                    self.frame_mut().chunk.write_byte(load_name, line);
                    self.emit_expr(value)?;
                    let arith = arith_for_compound(op);
                    self.frame_mut().chunk.write_op(arith, line);
                }
                let name_idx = self
                    .frame_mut()
                    .chunk
                    .add_constant(Value::Str(Rc::new(name.clone())));
                self.frame_mut().chunk.write_op(OpCode::SetField, line);
                self.frame_mut().chunk.write_byte(name_idx, line);
                self.frame_mut().chunk.write_op(OpCode::Pop, line);
            }
        }
        Ok(())
    }

    // --- expression emission ---

    fn emit_expr(&mut self, expr: &Expr) -> Result<(), CompileError> {
        match expr {
            Expr::Int { value, line, .. } => {
                let idx = self.frame_mut().chunk.add_constant(Value::Int(*value));
                self.frame_mut().chunk.write_op(OpCode::Constant, *line);
                self.frame_mut().chunk.write_byte(idx, *line);
            }
            Expr::Float { value, line, .. } => {
                let idx = self.frame_mut().chunk.add_constant(Value::Float(*value));
                self.frame_mut().chunk.write_op(OpCode::Constant, *line);
                self.frame_mut().chunk.write_byte(idx, *line);
            }
            Expr::Bool { value, line, .. } => {
                self.frame_mut().chunk.write_op(
                    if *value { OpCode::True } else { OpCode::False },
                    *line,
                );
            }
            Expr::Str { value, line, .. } => {
                let idx = self
                    .frame_mut()
                    .chunk
                    .add_constant(Value::Str(Rc::new(value.clone())));
                self.frame_mut().chunk.write_op(OpCode::Constant, *line);
                self.frame_mut().chunk.write_byte(idx, *line);
            }
            Expr::Percent { value, line, .. } => {
                let idx = self.frame_mut().chunk.add_constant(Value::Float(*value));
                self.frame_mut().chunk.write_op(OpCode::Constant, *line);
                self.frame_mut().chunk.write_byte(idx, *line);
            }
            Expr::Ident { name, line, .. } => {
                if let Some(slot) = self.resolve_local(name) {
                    self.frame_mut().chunk.write_op(OpCode::GetLocal, *line);
                    self.frame_mut().chunk.write_byte(slot, *line);
                } else if self.is_self_field(name) {
                    // Bare name resolves to `self.name` inside a
                    // method/state body when the class declares it.
                    self.frame_mut().chunk.write_op(OpCode::GetLocal, *line);
                    self.frame_mut().chunk.write_byte(0, *line);
                    let name_idx = self
                        .frame_mut()
                        .chunk
                        .add_constant(Value::Str(Rc::new(name.clone())));
                    self.frame_mut().chunk.write_op(OpCode::GetField, *line);
                    self.frame_mut().chunk.write_byte(name_idx, *line);
                } else {
                    let name_idx = self
                        .frame_mut()
                        .chunk
                        .add_constant(Value::Str(Rc::new(name.clone())));
                    self.frame_mut().chunk.write_op(OpCode::GetGlobal, *line);
                    self.frame_mut().chunk.write_byte(name_idx, *line);
                }
            }
            Expr::Unary { op, operand, line, .. } => {
                self.emit_expr(operand)?;
                match op {
                    UnOp::Neg => self.frame_mut().chunk.write_op(OpCode::Neg, *line),
                    UnOp::Not => self.frame_mut().chunk.write_op(OpCode::Not, *line),
                }
            }
            Expr::Binary { op, left, right, line, col: _ } => {
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
                    BinOp::In => {
                        // emit_expr already pushed left then right.
                        // OP_IN pops haystack then needle; left = needle,
                        // right = haystack — matches the source order.
                        self.frame_mut().chunk.write_op(OpCode::In, *line);
                        return Ok(());
                    }
                    BinOp::NotIn => {
                        self.frame_mut().chunk.write_op(OpCode::In, *line);
                        self.frame_mut().chunk.write_op(OpCode::Not, *line);
                        return Ok(());
                    }
                };
                self.frame_mut().chunk.write_op(op, *line);
            }
            Expr::Call { callee, args, kwargs, line, col } => {
                self.emit_call(callee, args, kwargs, *line, *col)?;
            }
            Expr::Tuple { elems, line, col } => {
                if elems.len() > u8::MAX as usize {
                    return Err(CompileError {
                        line: *line,
                        col: *col,
                        message: format!("tuple literal too large (max {})", u8::MAX),
                    });
                }
                for e in elems {
                    self.emit_expr(e)?;
                }
                self.frame_mut().chunk.write_op(OpCode::BuildTuple, *line);
                self.frame_mut().chunk.write_byte(elems.len() as u8, *line);
            }
            Expr::List { elems, line, col } => {
                if elems.len() > u8::MAX as usize {
                    return Err(CompileError {
                        line: *line,
                        col: *col,
                        message: format!("list literal too large (max {})", u8::MAX),
                    });
                }
                for e in elems {
                    self.emit_expr(e)?;
                }
                self.frame_mut().chunk.write_op(OpCode::BuildList, *line);
                self.frame_mut().chunk.write_byte(elems.len() as u8, *line);
            }
            Expr::Range { start, end, exclusive, line, .. } => {
                self.emit_expr(start)?;
                self.emit_expr(end)?;
                self.frame_mut().chunk.write_op(OpCode::BuildRange, *line);
                self.frame_mut().chunk.write_byte(if *exclusive { 1 } else { 0 }, *line);
            }
            Expr::Index { object, index, line, .. } => {
                self.emit_expr(object)?;
                self.emit_expr(index)?;
                self.frame_mut().chunk.write_op(OpCode::Index, *line);
            }
            Expr::Field { object, name, line, .. } => {
                self.emit_expr(object)?;
                let name_idx = self
                    .frame_mut()
                    .chunk
                    .add_constant(Value::Str(Rc::new(name.clone())));
                self.frame_mut().chunk.write_op(OpCode::GetField, *line);
                self.frame_mut().chunk.write_byte(name_idx, *line);
            }
            Expr::Interp { parts, exprs, line, col } => {
                // parts.len() == exprs.len() + 1 by construction in
                // `lex_string`. Emit alternating text-const + expr-as-str
                // so the VM can OP_INTERP them all into a single Str.
                let total = parts.len() + exprs.len();
                if total > u8::MAX as usize {
                    return Err(CompileError {
                        line: *line,
                        col: *col,
                        message: format!(
                            "interpolated string has too many parts ({total}, max {})",
                            u8::MAX
                        ),
                    });
                }
                for (i, p) in parts.iter().enumerate() {
                    let idx = self
                        .frame_mut()
                        .chunk
                        .add_constant(Value::Str(Rc::new(p.clone())));
                    self.frame_mut().chunk.write_op(OpCode::Constant, *line);
                    self.frame_mut().chunk.write_byte(idx, *line);
                    if let Some(e) = exprs.get(i) {
                        self.emit_expr(e)?;
                        self.frame_mut().chunk.write_op(OpCode::ToStr, e.line());
                    }
                }
                self.frame_mut().chunk.write_op(OpCode::Interp, *line);
                self.frame_mut().chunk.write_byte(total as u8, *line);
            }
            Expr::SelfRef { line, col } => {
                // `self` is the receiver, sitting at slot 0 of every
                // method's frame (Crafting Interpreters §28.5). Outside
                // a method body it's a compile-time error.
                if self.frame().kind == FrameKind::Script
                    || !self.frame().is_method
                {
                    return Err(CompileError {
                        line: *line,
                        col: *col,
                        message: "`self` is only valid inside a method body".to_string(),
                    });
                }
                self.frame_mut().chunk.write_op(OpCode::GetLocal, *line);
                self.frame_mut().chunk.write_byte(0, *line);
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

    /// Compile a call site. Method calls (`recv.name(args)`) lower to
    /// `OP_INVOKE name_idx argcount` so the VM can dispatch built-in
    /// receivers (lists, ranges) without first materialising the
    /// method as a Value. Plain calls go through `OP_CALL`.
    fn emit_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        kwargs: &[(String, Expr)],
        line: u32,
        col: u32,
    ) -> Result<(), CompileError> {
        if !kwargs.is_empty() {
            return Err(self.unsupported(
                "keyword arguments in bytecode calls (lands once builtin dispatch \
                 reaches the VM in session 11)",
                line,
                col,
            ));
        }
        if args.len() > u8::MAX as usize {
            return Err(CompileError {
                line,
                col,
                message: format!("call has too many arguments (max {})", u8::MAX),
            });
        }
        if let Expr::Field { object, name, line: fline, .. } = callee {
            // Method call. Push receiver + args, then OP_INVOKE.
            self.emit_expr(object)?;
            for arg in args {
                self.emit_expr(arg)?;
            }
            let name_idx = self
                .frame_mut()
                .chunk
                .add_constant(Value::Str(Rc::new(name.clone())));
            self.frame_mut().chunk.write_op(OpCode::Invoke, *fline);
            self.frame_mut().chunk.write_byte(name_idx, *fline);
            self.frame_mut().chunk.write_byte(args.len() as u8, *fline);
            return Ok(());
        }
        self.emit_expr(callee)?;
        for arg in args {
            self.emit_expr(arg)?;
        }
        self.frame_mut().chunk.write_op(OpCode::Call, line);
        self.frame_mut().chunk.write_byte(args.len() as u8, line);
        Ok(())
    }

    fn emit_and(&mut self, left: &Expr, right: &Expr, line: u32) -> Result<(), CompileError> {
        // `a and b`: emit a; if a is falsy, leave a as the result and
        // skip b. Otherwise pop a, evaluate b, and let b be the result.
        self.emit_expr(left)?;
        let end_jump = self.emit_jump(OpCode::JumpIfFalsePeek, line);
        self.frame_mut().chunk.write_op(OpCode::Pop, line);
        self.emit_expr(right)?;
        self.patch_jump(end_jump)?;
        Ok(())
    }

    fn emit_or(&mut self, left: &Expr, right: &Expr, line: u32) -> Result<(), CompileError> {
        // `a or b`: emit a; if a is truthy, leave a as the result and
        // skip b. Otherwise pop a, evaluate b, and let b be the result.
        self.emit_expr(left)?;
        let end_jump = self.emit_jump(OpCode::JumpIfTruePeek, line);
        self.frame_mut().chunk.write_op(OpCode::Pop, line);
        self.emit_expr(right)?;
        self.patch_jump(end_jump)?;
        Ok(())
    }

    // --- locals + scopes ---

    /// True when a top-level `let`/`var` should become a global.
    /// Only the script frame at scope-depth 0 promotes; everything
    /// else (function body, any inner block) takes the local path.
    fn is_global_scope(&self) -> bool {
        self.frame().kind == FrameKind::Script && self.frame().scope_depth == 0
    }

    fn declare_local(
        &mut self,
        name: &str,
        line: u32,
        col: u32,
    ) -> Result<(), CompileError> {
        let frame = self.frame();
        if frame.locals.len() >= 256 {
            return Err(CompileError {
                line,
                col,
                message: "too many local variables in this function (max 256)".to_string(),
            });
        }
        // Same-scope shadowing is forbidden; outer-scope shadowing is fine.
        for local in frame.locals.iter().rev() {
            if local.depth != -1 && local.depth < frame.scope_depth {
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
        self.frame_mut().locals.push(Local {
            name: name.to_string(),
            depth: -1,
        });
        Ok(())
    }

    fn mark_initialised(&mut self) {
        let depth = self.frame().scope_depth;
        if let Some(last) = self.frame_mut().locals.last_mut() {
            last.depth = depth;
        }
    }

    fn resolve_local(&self, name: &str) -> Option<u8> {
        let frame = self.frame();
        for (i, local) in frame.locals.iter().enumerate().rev() {
            if local.name == name && local.depth != -1 {
                return Some(i as u8);
            }
        }
        None
    }

    /// True when a bare name should rewrite to `self.name` — the
    /// active frame is a method/state body and `name` is one of
    /// the enclosing class's declared fields. Mirrors the tree-
    /// walker's `lookup_name` self-field fallback.
    fn is_self_field(&self, name: &str) -> bool {
        if !self.frame().is_method {
            return false;
        }
        match &self.frame().class_fields {
            Some(set) => set.contains(name),
            None => false,
        }
    }

    fn begin_scope(&mut self) {
        self.frame_mut().scope_depth += 1;
    }

    fn end_scope(&mut self, line: u32) {
        self.frame_mut().scope_depth -= 1;
        let target_depth = self.frame().scope_depth;
        while let Some(last) = self.frame().locals.last() {
            if last.depth > target_depth {
                self.frame_mut().chunk.write_op(OpCode::Pop, line);
                self.frame_mut().locals.pop();
            } else {
                break;
            }
        }
    }

    // --- jump emission and patching ---

    fn emit_jump(&mut self, op: OpCode, line: u32) -> usize {
        let chunk = &mut self.frame_mut().chunk;
        chunk.write_op(op, line);
        chunk.write_byte(0xFF, line);
        chunk.write_byte(0xFF, line);
        chunk.code.len() - 2
    }

    fn patch_jump(&mut self, offset: usize) -> Result<(), CompileError> {
        let chunk = &mut self.frame_mut().chunk;
        let jump = chunk.code.len() - offset - 2;
        if jump > u16::MAX as usize {
            return Err(CompileError {
                line: chunk.lines[offset],
                col: 0,
                message: format!(
                    "jump distance {jump} exceeds 16-bit limit"
                ),
            });
        }
        chunk.code[offset] = ((jump >> 8) & 0xFF) as u8;
        chunk.code[offset + 1] = (jump & 0xFF) as u8;
        Ok(())
    }

    fn emit_loop(&mut self, loop_start: usize, line: u32) -> Result<(), CompileError> {
        let chunk = &mut self.frame_mut().chunk;
        chunk.write_op(OpCode::Loop, line);
        // Distance = current_ip + 2 (operand) - loop_start, then the
        // VM subtracts that from post-operand IP to land at loop_start.
        let jump = chunk.code.len() - loop_start + 2;
        if jump > u16::MAX as usize {
            return Err(CompileError {
                line,
                col: 0,
                message: format!(
                    "loop body too large for 16-bit OP_LOOP offset ({jump})"
                ),
            });
        }
        chunk.write_byte(((jump >> 8) & 0xFF) as u8, line);
        chunk.write_byte((jump & 0xFF) as u8, line);
        Ok(())
    }

    fn unsupported(&self, what: &str, line: u32, col: u32) -> CompileError {
        CompileError {
            line,
            col,
            message: format!("bytecode compiler doesn't yet support {what}"),
        }
    }

    /// Compile a class declaration (`entity` / `item` / `modifier`
    /// / `inventory` / `scene`). Two-pass: first collect field
    /// names + defaults, then compile methods + states with the
    /// field set available so bare names inside method/state
    /// bodies can route through `self.field`. Particles need
    /// lifetime tracking that lands in session 13.
    fn emit_decl(
        &mut self,
        kind: DeclKind,
        name: &str,
        parent: Option<&str>,
        members: &[DeclMember],
        line: u32,
        col: u32,
    ) -> Result<(), CompileError> {
        if parent.is_some() {
            return Err(self.unsupported(
                "class inheritance (parent `extends ...`)",
                line,
                col,
            ));
        }
        if !self.is_global_scope() {
            return Err(CompileError {
                line,
                col,
                message: format!(
                    "`{}` declarations are only supported at top level in v0.1",
                    kind.as_str()
                ),
            });
        }

        // Pass 1: scan fields. Const-fold each default. Track names.
        let mut field_defaults: HashMap<String, Value> = HashMap::new();
        let mut field_names: HashSet<String> = HashSet::new();
        let mut initial_state: Option<String> = None;
        for member in members {
            match member {
                DeclMember::Field { name: fname, value, line: fline, col: fcol } => {
                    let v = const_eval(value).ok_or_else(|| CompileError {
                        line: *fline,
                        col: *fcol,
                        message: format!(
                            "field `{fname}` default must be a literal constant in v0.1 \
                             (int, float, bool, string, percent, or a tuple of those)"
                        ),
                    })?;
                    field_defaults.insert(fname.clone(), v);
                    field_names.insert(fname.clone());
                }
                DeclMember::InitialState { name: sname, .. } => {
                    initial_state = Some(sname.clone());
                }
                _ => {}
            }
        }
        let class_fields = Rc::new(field_names);

        // Pass 2: compile methods + states with class_fields known.
        let mut methods: HashMap<String, Rc<BcFunction>> = HashMap::new();
        let mut states: HashMap<String, Rc<BcStateDef>> = HashMap::new();
        for member in members {
            match member {
                DeclMember::Field { .. } | DeclMember::InitialState { .. } => {}
                DeclMember::Method { name: mname, params, body, line: mline, col: mcol } => {
                    let func = self.compile_method(
                        mname,
                        params,
                        body,
                        class_fields.clone(),
                        *mline,
                        *mcol,
                    )?;
                    methods.insert(mname.clone(), Rc::new(func));
                }
                DeclMember::State { name: sname, members: smembers, line: sline, col: scol } => {
                    let state = self.compile_state(
                        sname,
                        smembers,
                        class_fields.clone(),
                        *sline,
                        *scol,
                    )?;
                    states.insert(sname.clone(), Rc::new(state));
                }
            }
        }

        if let Some(start) = &initial_state {
            if !states.contains_key(start) {
                return Err(CompileError {
                    line,
                    col,
                    message: format!(
                        "`initial: {start}` references a state that doesn't exist on `{name}`"
                    ),
                });
            }
        }

        let class = Rc::new(BcClassDef {
            kind: kind.as_str(),
            name: name.to_string(),
            field_defaults,
            methods,
            states,
            initial_state,
        });
        let class_idx = self
            .frame_mut()
            .chunk
            .add_constant(Value::BcClass(class));
        self.frame_mut().chunk.write_op(OpCode::Constant, line);
        self.frame_mut().chunk.write_byte(class_idx, line);
        let name_idx = self
            .frame_mut()
            .chunk
            .add_constant(Value::Str(Rc::new(name.to_string())));
        self.frame_mut().chunk.write_op(OpCode::DefineGlobal, line);
        self.frame_mut().chunk.write_byte(name_idx, line);
        // Scene auto-instantiation happens at the VM, triggered by
        // OP_INIT_SCENE which runs immediately after the class is
        // bound. Keeps the runtime spawn logic out of compile time.
        if matches!(kind, DeclKind::Scene) {
            self.frame_mut().chunk.write_op(OpCode::GetGlobal, line);
            self.frame_mut().chunk.write_byte(name_idx, line);
            self.frame_mut().chunk.write_op(OpCode::InitScene, line);
        }
        Ok(())
    }

    fn compile_method(
        &mut self,
        name: &str,
        params: &[String],
        body: &[Stmt],
        class_fields: Rc<HashSet<String>>,
        line: u32,
        col: u32,
    ) -> Result<BcFunction, CompileError> {
        if params.len() > u8::MAX as usize {
            return Err(CompileError {
                line,
                col,
                message: format!(
                    "method `{name}` has too many parameters (max {})",
                    u8::MAX
                ),
            });
        }
        let arity = params.len() as u8;
        self.frames.push(Frame::with_method(
            FrameKind::Function,
            name,
            arity,
            true,
            Some(class_fields),
        ));
        // Slot 0 is `self`; params live at slots 1..=arity.
        for param in params {
            self.declare_local(param, line, col)?;
            self.mark_initialised();
        }
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        let last_line = body.last().map(stmt_line).unwrap_or(line);
        self.frame_mut().chunk.write_op(OpCode::Nil, last_line);
        self.frame_mut().chunk.write_op(OpCode::Return, last_line);
        let frame = self.frames.pop().expect("method frame we just pushed");
        Ok(BcFunction::new(frame.name, frame.arity, frame.chunk))
    }

    /// Compile a state member set into a `BcStateDef`. on_render
    /// and on_key_press defer to session 13; they're recognised
    /// here so the parser still accepts them, but emit a clear
    /// "not yet" error.
    fn compile_state(
        &mut self,
        name: &str,
        members: &[StateMember],
        class_fields: Rc<HashSet<String>>,
        line: u32,
        _col: u32,
    ) -> Result<BcStateDef, CompileError> {
        let mut on_entry_stmts: Vec<Stmt> = Vec::new();
        let mut every: Vec<(f64, Rc<BcFunction>)> = Vec::new();
        let mut on_update: Option<Rc<BcFunction>> = None;
        let mut on_render: Option<Rc<BcFunction>> = None;
        let mut on_key_press: HashMap<String, Rc<BcFunction>> = HashMap::new();
        for sm in members {
            match sm {
                StateMember::Stmt(s) => on_entry_stmts.push(s.clone()),
                StateMember::Every { interval, body, line: el, col: ec } => {
                    let secs = const_eval_seconds(interval).ok_or_else(|| CompileError {
                        line: *el,
                        col: *ec,
                        message: "`every <duration>:` interval must be a literal duration \
                                  in v0.1 (e.g. `100ms`, `0.5s`)".to_string(),
                    })?;
                    let func = self.compile_state_body(
                        &format!("{name}.every"),
                        body,
                        class_fields.clone(),
                        *el,
                        *ec,
                    )?;
                    every.push((secs, Rc::new(func)));
                }
                StateMember::OnUpdate { param, body, line: ul, col: uc } => {
                    let func = self.compile_state_on_update(
                        &format!("{name}.on_update"),
                        param,
                        body,
                        class_fields.clone(),
                        *ul,
                        *uc,
                    )?;
                    on_update = Some(Rc::new(func));
                }
                StateMember::OnRender { body, line: rl, col: _rc } => {
                    let func = self.compile_state_body(
                        &format!("{name}.on_render"),
                        body,
                        class_fields.clone(),
                        *rl,
                        0,
                    )?;
                    on_render = Some(Rc::new(func));
                }
                StateMember::OnKeyPress { key, body, line: kl, col: _kc } => {
                    let func = self.compile_state_body(
                        &format!("{name}.on_key_press.{key}"),
                        body,
                        class_fields.clone(),
                        *kl,
                        0,
                    )?;
                    on_key_press.insert(key.clone(), Rc::new(func));
                }
            }
        }
        let on_entry = self.compile_state_body(
            &format!("{name}.on_entry"),
            &on_entry_stmts,
            class_fields,
            line,
            0,
        )?;
        Ok(BcStateDef {
            name: name.to_string(),
            on_entry: Rc::new(on_entry),
            every_clocks: every,
            on_update,
            on_render,
            on_key_press,
        })
    }

    /// Compile a state-scoped body (on_entry, every-clock body,
    /// state on_update body) as a method-shape BcFunction with
    /// `self` at slot 0. on_update bodies take a `dt` parameter
    /// at slot 1.
    fn compile_state_body(
        &mut self,
        name: &str,
        body: &[Stmt],
        class_fields: Rc<HashSet<String>>,
        line: u32,
        _col: u32,
    ) -> Result<BcFunction, CompileError> {
        self.frames.push(Frame::with_method(
            FrameKind::Function,
            name,
            0,
            true,
            Some(class_fields),
        ));
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        let last_line = body.last().map(stmt_line).unwrap_or(line);
        self.frame_mut().chunk.write_op(OpCode::Nil, last_line);
        self.frame_mut().chunk.write_op(OpCode::Return, last_line);
        let frame = self.frames.pop().expect("state-body frame we just pushed");
        Ok(BcFunction::new(frame.name, frame.arity, frame.chunk))
    }

    /// Compile a top-level `on update(dt):` body as a non-method
    /// function: slot 0 is the function value (unused by body),
    /// slot 1 is `dt`. Globals reachable, no `self`.
    fn compile_top_level_on_update(
        &mut self,
        param: &str,
        body: &[Stmt],
        line: u32,
        col: u32,
    ) -> Result<BcFunction, CompileError> {
        self.frames
            .push(Frame::new(FrameKind::Function, "<on_update>", 1));
        self.declare_local(param, line, col)?;
        self.mark_initialised();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        let last_line = body.last().map(stmt_line).unwrap_or(line);
        self.frame_mut().chunk.write_op(OpCode::Nil, last_line);
        self.frame_mut().chunk.write_op(OpCode::Return, last_line);
        let frame = self.frames.pop().expect("on_update frame we just pushed");
        Ok(BcFunction::new(frame.name, frame.arity, frame.chunk))
    }

    fn compile_state_on_update(
        &mut self,
        name: &str,
        param: &str,
        body: &[Stmt],
        class_fields: Rc<HashSet<String>>,
        line: u32,
        col: u32,
    ) -> Result<BcFunction, CompileError> {
        self.frames.push(Frame::with_method(
            FrameKind::Function,
            name,
            1,
            true,
            Some(class_fields),
        ));
        self.declare_local(param, line, col)?;
        self.mark_initialised();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        let last_line = body.last().map(stmt_line).unwrap_or(line);
        self.frame_mut().chunk.write_op(OpCode::Nil, last_line);
        self.frame_mut().chunk.write_op(OpCode::Return, last_line);
        let frame = self.frames.pop().expect("state on_update frame we just pushed");
        Ok(BcFunction::new(frame.name, frame.arity, frame.chunk))
    }
}

/// Constant-evaluate an `every` interval to seconds. Accepts a
/// duration literal (`100ms`, `0.5s`, `2min`, `1h`) or a numeric
/// literal (interpreted as seconds). Returns `None` for anything
/// that depends on runtime state — the compiler then errors with
/// a pointer to the limitation.
fn const_eval_seconds(e: &Expr) -> Option<f64> {
    match e {
        Expr::Quantity { value, unit, .. } => match unit.as_str() {
            "s" => Some(*value),
            "ms" => Some(*value / 1000.0),
            "min" => Some(*value * 60.0),
            "h" => Some(*value * 3600.0),
            _ => None,
        },
        Expr::Float { value, .. } => Some(*value),
        Expr::Int { value, .. } => Some(*value as f64),
        _ => None,
    }
}

/// Constant-evaluate an expression for use as a field default.
/// Mirrors a small literal subset of `eval::eval_expr`. Returns
/// `None` for any expression that depends on runtime state.
fn const_eval(e: &Expr) -> Option<Value> {
    match e {
        Expr::Int { value, .. } => Some(Value::Int(*value)),
        Expr::Float { value, .. } => Some(Value::Float(*value)),
        Expr::Bool { value, .. } => Some(Value::Bool(*value)),
        Expr::Str { value, .. } => Some(Value::Str(Rc::new(value.clone()))),
        Expr::Percent { value, .. } => Some(Value::Percent(*value)),
        Expr::Tuple { elems, .. } => {
            let vals: Option<Vec<Value>> = elems.iter().map(const_eval).collect();
            vals.map(|v| Value::Tuple(Rc::new(v)))
        }
        Expr::Unary { op: UnOp::Neg, operand, .. } => match const_eval(operand)? {
            Value::Int(n) => Some(Value::Int(-n)),
            Value::Float(x) => Some(Value::Float(-x)),
            _ => None,
        },
        _ => None,
    }
}

fn arith_for_compound(op: AssignOp) -> OpCode {
    match op {
        AssignOp::AddAssign => OpCode::Add,
        AssignOp::SubAssign => OpCode::Sub,
        AssignOp::MulAssign => OpCode::Mul,
        AssignOp::DivAssign => OpCode::Div,
        AssignOp::Set => unreachable!("Set is handled separately"),
    }
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
    fn top_level_let_then_use_emits_globals() {
        // Session 9: top-level `let x = 5` lowers to OP_DEFINE_GLOBAL,
        // and a later `print(x)` uses OP_GET_GLOBAL — the locals path
        // is reserved for inner scopes and function bodies.
        let prog = parse_program("let x = 5\nprint(x)\n");
        let chunk = compile_program(&prog).expect("compile");
        assert_eq!(
            chunk.code,
            vec![
                OpCode::Constant as u8, 0,        // 5
                OpCode::DefineGlobal as u8, 1,    // "x"
                OpCode::GetGlobal as u8, 2,       // "x" again
                OpCode::Print as u8,
                OpCode::Return as u8,
            ]
        );
    }

    #[test]
    fn undefined_name_compiles_and_defers_to_runtime() {
        // Session 9 change: `print(missing)` is legal at compile time
        // because `missing` is treated as a global lookup. The error
        // moves to runtime (VM raises "global 'missing' is not defined").
        let chunk = compile_program(&parse_program("print(missing)\n")).expect("compile");
        // Should contain OP_GET_GLOBAL.
        assert!(
            chunk.code.contains(&(OpCode::GetGlobal as u8)),
            "expected OP_GET_GLOBAL: {:?}",
            chunk.code
        );
    }

    #[test]
    fn compile_short_circuit_emits_jump_pattern() {
        // `a and b` → emit a, JUMP_IF_FALSE_PEEK over (POP + b), patch.
        let prog = parse_program("let a = true\nlet b = false\nprint(a and b)\n");
        let chunk = compile_program(&prog).expect("compile");
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
    fn compile_function_decl_emits_constant_and_define_global() {
        let prog = parse_program("function greet():\n    print(\"hi\")\n");
        let chunk = compile_program(&prog).expect("compile");
        // The outer chunk should hold the BcFunction as a constant,
        // followed by OP_DEFINE_GLOBAL for the name.
        let has_function_constant = chunk.constants.iter().any(|v| matches!(v, Value::BcFunction(_)));
        assert!(has_function_constant, "expected a BcFunction constant: {:?}", chunk.constants);
        assert!(
            chunk.code.contains(&(OpCode::DefineGlobal as u8)),
            "expected OP_DEFINE_GLOBAL: {:?}",
            chunk.code
        );
    }

    #[test]
    fn compile_call_emits_call_op() {
        let prog = parse_program(
            "function add(a, b):\n    return a + b\n\nlet r = add(2, 3)\nprint(r)\n",
        );
        let chunk = compile_program(&prog).expect("compile");
        // The script chunk should contain at least one OP_CALL.
        assert!(
            chunk.code.contains(&(OpCode::Call as u8)),
            "expected OP_CALL: {:?}",
            chunk.code
        );
    }

    #[test]
    fn return_outside_function_errors() {
        let err = compile_program(&parse_program("return 1\n")).expect_err("err");
        assert!(
            err.message.contains("`return`"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn nested_function_decl_errors() {
        // Closures aren't in v0.1 — declaring a function inside a
        // function should fail at compile time with a clear message.
        let src = "function outer():\n    function inner():\n        print(1)\n";
        let err = compile_program(&parse_program(src)).expect_err("err");
        assert!(
            err.message.contains("top level"),
            "got: {}",
            err.message
        );
    }
}
