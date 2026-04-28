//! Bytecode representation for the Phase-3 VM.
//!
//! Opcodes, the `Chunk` struct (compiled code, constants, and
//! per-byte line info), the `BcFunction` wrapper that names a chunk
//! and records arity, and a disassembler. The compiler (AST →
//! bytecode) lives in `crate::compiler`; the dispatch loop lives in
//! `crate::vm`. The tree-walker in `crate::eval` remains the active
//! interpreter for the CLI until the bytecode VM reaches feature
//! parity (around session 11).
//!
//! Design follows *Crafting Interpreters* Chapter 14 closely. We
//! reuse `crate::value::Value` for the constant pool rather than
//! introducing a separate "compiled value" type — Phase-3 NaN
//! tagging is a later session and changes Value globally when it
//! lands. Until then, Rc-clones on Value are cheap enough for the
//! bytecode pool.

use std::collections::HashMap;
use std::rc::Rc;

use crate::value::Value;

/// One instruction in the Twe bytecode. Each variant fits in a u8.
/// Variants that take operands consume additional bytes from the
/// `code` stream — for now only `Constant` (one u8 = constant pool
/// index, max 256 constants per chunk in this draft).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    /// Push a constant from the chunk's constant pool. 1-byte operand
    /// is the constant index.
    Constant = 0,
    /// Push the literal value `nil`.
    Nil,
    /// Push the literal value `true`.
    True,
    /// Push the literal value `false`.
    False,
    /// Pop the top of the stack and discard it. Used at end of
    /// statement-expressions.
    Pop,

    // Binary arithmetic — pop two, push one.
    Add,
    Sub,
    Mul,
    Div,
    /// Modulo (`%`).
    Mod,

    /// Unary negation.
    Neg,
    /// Boolean `not` — pops, pushes strict-Bool inversion of truthiness.
    Not,

    // Comparisons — pop two, push one Bool.
    Equal,
    NotEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,

    /// Print top-of-stack with a trailing newline (placeholder until the
    /// `print` builtin is reachable through bytecode call sites).
    Print,

    /// Return from the current function. The dispatch loop ends when
    /// this fires at the top level.
    Return,

    // --- Session 8: locals + control flow ---

    /// Push the local at stack slot N (1-byte operand). Locals live
    /// at the bottom of the stack; the compiler tracks names → slots
    /// at compile time.
    GetLocal,
    /// Set the local at stack slot N to the value on top of stack.
    /// Leaves the value on the stack so it can be the result of an
    /// assignment expression.
    SetLocal,
    /// Pop top of stack; if it's falsy, jump forward by the unsigned
    /// 16-bit operand (big-endian, two bytes after the opcode).
    /// Pre-pop variant used by `if` and `while` conditions; the
    /// short-circuit-preserving variants (`and`/`or`) use
    /// `JumpIfFalsePeek` so the operand stays for use as the
    /// expression's value.
    JumpIfFalse,
    /// Like `JumpIfFalse` but does NOT pop the condition; used by
    /// `and` (jump if left is false, leaving left as the result).
    JumpIfFalsePeek,
    /// Like `JumpIfFalsePeek` but for `or` (jump if left is truthy).
    JumpIfTruePeek,
    /// Unconditional forward jump. 2-byte unsigned big-endian
    /// operand.
    Jump,
    /// Unconditional backward jump for loops. 2-byte unsigned
    /// big-endian operand subtracted from the post-operand IP.
    Loop,

    // --- Session 9: globals + functions + calls ---

    /// Pop top of stack and bind it to the global named by the
    /// constant-pool string at the 1-byte operand index. Used by
    /// top-level `let` / `var` and by function declarations.
    DefineGlobal,
    /// Push the value of the global named by the constant-pool
    /// string at the 1-byte operand index. Runtime error if the
    /// name has no binding.
    GetGlobal,
    /// Set the global named by the constant-pool string at the
    /// 1-byte operand index to the value on top of stack. Leaves
    /// the value on the stack so the assignment expression's
    /// caller can pop it. Runtime error if the global doesn't
    /// already exist (assignment requires prior `let`/`var`).
    SetGlobal,
    /// Call the value at `stack[top - arg_count]` with the
    /// `arg_count` values above it. 1-byte operand is the arg
    /// count. The callee pushes a new CallFrame; the args remain
    /// on the stack as the new frame's locals 1..=arg_count, with
    /// the function value itself at the new frame's local 0.
    Call,

    // --- Session 10: heap types + for-loops ---

    /// Pop `n` values and push them as a `Value::Tuple`. 1-byte
    /// operand is `n`. The first popped value is the *last*
    /// element; the compiler emits values left-to-right so the
    /// VM reverses on pop.
    BuildTuple,
    /// Pop `n` values and push them as a `Value::List`. Same
    /// ordering convention as `BuildTuple`.
    BuildList,
    /// Pop end then start, push a `Value::Range`. 1-byte operand
    /// is the inclusivity flag: 0 = inclusive (`..`), 1 =
    /// exclusive (`..<`). Both bounds must be ints (runtime check
    /// mirrors `eval`).
    BuildRange,
    /// Pop index then container, push the indexed element. Works
    /// on List and Tuple; errors on other types. Negative indices
    /// count from the end (matches `eval::index_get`).
    Index,
    /// Pop one value, push its display form as a `Value::Str`.
    /// Used by string interpolation to coerce numbers / tuples /
    /// nested values into renderable text.
    ToStr,
    /// Pop `n` strings and push their concatenation as a single
    /// `Value::Str`. 1-byte operand is `n`. Compiler emits the
    /// alternating text + expr-as-str sequence in source order;
    /// VM concatenates in order.
    Interp,
    /// Pop the haystack then the needle, push a `Value::Bool`
    /// indicating membership. Mirrors `eval::value_in` for List,
    /// Tuple, Range, and substring-in-Str.
    In,
    /// Read a field by name (constant-pool index, 1 byte) from
    /// the value on top of stack and push the result. Tuple
    /// exposes `.x` `.y` `.z`; List exposes `.length`. Other
    /// receivers error — Object and Instance fields land with
    /// the declarative-blocks pass in session 11.
    GetField,
    /// Method call. Operands: 1-byte name index in the constant
    /// pool, 1-byte arg count. Stack on entry: `[..., recv, arg1,
    /// ..., argN]`. Dispatches to built-in list/range methods;
    /// other receivers error pending session 11.
    Invoke,
    /// Iterate one step of a `for var in iter:` loop.
    /// Operands: 1-byte `base_slot` (the iterable's slot — the
    /// counter sits at `base_slot + 1`), 2-byte big-endian exit
    /// jump offset. Behaviour: if the iterable is exhausted, jump
    /// to exit; otherwise increment the counter and push the next
    /// element on the stack (the compiler binds it to the user's
    /// loop variable as the next local). Range / List / Tuple
    /// receivers; mirrors `eval::run_for`.
    ForNext,

    // --- Session 11: classes + instances + module builtins ---

    /// Set a field by name (constant-pool index, 1 byte) on the
    /// instance / object two-down on the stack to the value on
    /// top. Stack on entry: `[..., recv, value]`. Stack after:
    /// `[..., value]` — the assignment expression keeps its value
    /// for the statement caller's `OP_POP`. BcInstance and Object
    /// receivers; other types error.
    SetField,
}

impl OpCode {
    /// Decode a u8 read from a `Chunk::code` stream into an OpCode.
    /// Panics if the byte is not a valid opcode — bytecode is produced
    /// by the compiler in this crate and never read from disk in v0.1,
    /// so an invalid byte is a compiler bug rather than untrusted input.
    pub fn from_u8(byte: u8) -> Self {
        match byte {
            0 => OpCode::Constant,
            1 => OpCode::Nil,
            2 => OpCode::True,
            3 => OpCode::False,
            4 => OpCode::Pop,
            5 => OpCode::Add,
            6 => OpCode::Sub,
            7 => OpCode::Mul,
            8 => OpCode::Div,
            9 => OpCode::Mod,
            10 => OpCode::Neg,
            11 => OpCode::Not,
            12 => OpCode::Equal,
            13 => OpCode::NotEqual,
            14 => OpCode::Greater,
            15 => OpCode::GreaterEqual,
            16 => OpCode::Less,
            17 => OpCode::LessEqual,
            18 => OpCode::Print,
            19 => OpCode::Return,
            20 => OpCode::GetLocal,
            21 => OpCode::SetLocal,
            22 => OpCode::JumpIfFalse,
            23 => OpCode::JumpIfFalsePeek,
            24 => OpCode::JumpIfTruePeek,
            25 => OpCode::Jump,
            26 => OpCode::Loop,
            27 => OpCode::DefineGlobal,
            28 => OpCode::GetGlobal,
            29 => OpCode::SetGlobal,
            30 => OpCode::Call,
            31 => OpCode::BuildTuple,
            32 => OpCode::BuildList,
            33 => OpCode::BuildRange,
            34 => OpCode::Index,
            35 => OpCode::ToStr,
            36 => OpCode::Interp,
            37 => OpCode::In,
            38 => OpCode::GetField,
            39 => OpCode::Invoke,
            40 => OpCode::ForNext,
            41 => OpCode::SetField,
            other => panic!("OpCode::from_u8: invalid byte {other}"),
        }
    }
}

/// One compiled function body: a stream of bytecode bytes, the
/// constants those bytes reference, and a parallel line-number array
/// for error reporting. The line vec has one entry per byte in `code`,
/// not per instruction — keeps decoding simple at the cost of some
/// memory.
#[derive(Debug, Default, Clone)]
pub struct Chunk {
    pub code: Vec<u8>,
    pub constants: Vec<Value>,
    pub lines: Vec<u32>,
}

impl Chunk {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a single byte to the code stream with its source line.
    pub fn write_byte(&mut self, byte: u8, line: u32) {
        self.code.push(byte);
        self.lines.push(line);
    }

    /// Append an opcode with its source line.
    pub fn write_op(&mut self, op: OpCode, line: u32) {
        self.write_byte(op as u8, line);
    }

    /// Add a constant to the pool and return its index. Caller emits
    /// `OpCode::Constant` followed by this index as a u8 operand.
    /// Panics if the pool grows past 256 entries — that's a v0.1
    /// limit we'll lift when the compiler grows beyond toy programs
    /// (Crafting Interpreters Ch. 21 covers OP_CONSTANT_LONG).
    pub fn add_constant(&mut self, value: Value) -> u8 {
        let idx = self.constants.len();
        if idx >= 256 {
            panic!("chunk constant pool exceeded 256 entries");
        }
        self.constants.push(value);
        idx as u8
    }
}

/// A compiled function: a name (for diagnostics + display), a fixed
/// arity, and the chunk that runs when the function is called. The
/// VM wraps every chunk it executes in a `BcFunction` — even the
/// top-level script gets a synthetic one named `<script>` with
/// arity 0, so the dispatch loop can be uniformly frame-based per
/// *Crafting Interpreters* §24.4.
#[derive(Debug, Clone)]
pub struct BcFunction {
    pub name: String,
    pub arity: u8,
    pub chunk: Chunk,
}

impl BcFunction {
    pub fn new(name: impl Into<String>, arity: u8, chunk: Chunk) -> Self {
        Self {
            name: name.into(),
            arity,
            chunk,
        }
    }
}

/// A bytecode-VM class. Mirrors `value::ClassDef` but stores
/// methods as compiled `BcFunction`s and only carries the subset
/// of class state the bytecode VM uses today: name, kind (entity
/// / item / modifier / inventory), default fields, and methods.
/// State machines, initial states, every-clocks, and the rest of
/// the scene/particles surface area arrive in session 12 with the
/// play-loop integration.
#[derive(Debug)]
pub struct BcClassDef {
    pub kind: &'static str,
    pub name: String,
    pub field_defaults: HashMap<String, Value>,
    pub methods: HashMap<String, Rc<BcFunction>>,
}

/// A live instance of a `BcClassDef`. Field reads/writes go
/// through the inner `RefCell` so methods can mutate `self.x`.
#[derive(Debug)]
pub struct BcInstance {
    pub class: Rc<BcClassDef>,
    pub fields: HashMap<String, Value>,
}

/// Format a Chunk as a human-readable instruction listing. Mirrors
/// the style from *Crafting Interpreters* §14.6: 4-digit offset,
/// source line (`|` repeats), opcode name, and an operand value
/// where applicable.
pub fn disassemble(chunk: &Chunk, name: &str) -> String {
    let mut out = format!("== {name} ==\n");
    let mut offset = 0;
    while offset < chunk.code.len() {
        offset = disassemble_instruction(&mut out, chunk, offset);
    }
    out
}

/// Disassemble one instruction starting at `offset`; append the
/// formatted line to `out` and return the offset of the next
/// instruction. Public so test harnesses and a future `twec
/// disasm` CLI subcommand can step instruction-by-instruction.
pub fn disassemble_instruction(out: &mut String, chunk: &Chunk, offset: usize) -> usize {
    use std::fmt::Write;
    let _ = write!(out, "{offset:04} ");
    let line = chunk.lines[offset];
    if offset > 0 && line == chunk.lines[offset - 1] {
        out.push_str("   | ");
    } else {
        let _ = write!(out, "{line:>4} ");
    }
    let op = OpCode::from_u8(chunk.code[offset]);
    match op {
        OpCode::Constant => constant_instruction(out, "OP_CONSTANT", chunk, offset),
        OpCode::Nil => simple_instruction(out, "OP_NIL", offset),
        OpCode::True => simple_instruction(out, "OP_TRUE", offset),
        OpCode::False => simple_instruction(out, "OP_FALSE", offset),
        OpCode::Pop => simple_instruction(out, "OP_POP", offset),
        OpCode::Add => simple_instruction(out, "OP_ADD", offset),
        OpCode::Sub => simple_instruction(out, "OP_SUB", offset),
        OpCode::Mul => simple_instruction(out, "OP_MUL", offset),
        OpCode::Div => simple_instruction(out, "OP_DIV", offset),
        OpCode::Mod => simple_instruction(out, "OP_MOD", offset),
        OpCode::Neg => simple_instruction(out, "OP_NEG", offset),
        OpCode::Not => simple_instruction(out, "OP_NOT", offset),
        OpCode::Equal => simple_instruction(out, "OP_EQUAL", offset),
        OpCode::NotEqual => simple_instruction(out, "OP_NOT_EQUAL", offset),
        OpCode::Greater => simple_instruction(out, "OP_GREATER", offset),
        OpCode::GreaterEqual => simple_instruction(out, "OP_GREATER_EQUAL", offset),
        OpCode::Less => simple_instruction(out, "OP_LESS", offset),
        OpCode::LessEqual => simple_instruction(out, "OP_LESS_EQUAL", offset),
        OpCode::Print => simple_instruction(out, "OP_PRINT", offset),
        OpCode::Return => simple_instruction(out, "OP_RETURN", offset),
        OpCode::GetLocal => byte_instruction(out, "OP_GET_LOCAL", chunk, offset),
        OpCode::SetLocal => byte_instruction(out, "OP_SET_LOCAL", chunk, offset),
        OpCode::JumpIfFalse => jump_instruction(out, "OP_JUMP_IF_FALSE", 1, chunk, offset),
        OpCode::JumpIfFalsePeek => {
            jump_instruction(out, "OP_JUMP_IF_FALSE_PEEK", 1, chunk, offset)
        }
        OpCode::JumpIfTruePeek => {
            jump_instruction(out, "OP_JUMP_IF_TRUE_PEEK", 1, chunk, offset)
        }
        OpCode::Jump => jump_instruction(out, "OP_JUMP", 1, chunk, offset),
        OpCode::Loop => jump_instruction(out, "OP_LOOP", -1, chunk, offset),
        OpCode::DefineGlobal => constant_instruction(out, "OP_DEFINE_GLOBAL", chunk, offset),
        OpCode::GetGlobal => constant_instruction(out, "OP_GET_GLOBAL", chunk, offset),
        OpCode::SetGlobal => constant_instruction(out, "OP_SET_GLOBAL", chunk, offset),
        OpCode::Call => byte_instruction(out, "OP_CALL", chunk, offset),
        OpCode::BuildTuple => byte_instruction(out, "OP_BUILD_TUPLE", chunk, offset),
        OpCode::BuildList => byte_instruction(out, "OP_BUILD_LIST", chunk, offset),
        OpCode::BuildRange => byte_instruction(out, "OP_BUILD_RANGE", chunk, offset),
        OpCode::Index => simple_instruction(out, "OP_INDEX", offset),
        OpCode::ToStr => simple_instruction(out, "OP_TO_STR", offset),
        OpCode::Interp => byte_instruction(out, "OP_INTERP", chunk, offset),
        OpCode::In => simple_instruction(out, "OP_IN", offset),
        OpCode::GetField => constant_instruction(out, "OP_GET_FIELD", chunk, offset),
        OpCode::Invoke => invoke_instruction(out, "OP_INVOKE", chunk, offset),
        OpCode::ForNext => for_next_instruction(out, "OP_FOR_NEXT", chunk, offset),
        OpCode::SetField => constant_instruction(out, "OP_SET_FIELD", chunk, offset),
    }
}

fn invoke_instruction(out: &mut String, name: &str, chunk: &Chunk, offset: usize) -> usize {
    use std::fmt::Write;
    let name_idx = chunk.code[offset + 1];
    let arg_count = chunk.code[offset + 2];
    let method = chunk
        .constants
        .get(name_idx as usize)
        .map(Value::display)
        .unwrap_or_else(|| "<missing>".to_string());
    let _ = writeln!(out, "{name:<16} {name_idx:>4} '{method}' ({arg_count} args)");
    offset + 3
}

fn for_next_instruction(out: &mut String, name: &str, chunk: &Chunk, offset: usize) -> usize {
    use std::fmt::Write;
    let base_slot = chunk.code[offset + 1];
    let hi = chunk.code[offset + 2] as u16;
    let lo = chunk.code[offset + 3] as u16;
    let jump = (hi << 8) | lo;
    let target = (offset + 4) as i32 + jump as i32;
    let _ = writeln!(out, "{name:<16} base={base_slot} -> {target}");
    offset + 4
}

fn byte_instruction(out: &mut String, name: &str, chunk: &Chunk, offset: usize) -> usize {
    use std::fmt::Write;
    let slot = chunk.code[offset + 1];
    let _ = writeln!(out, "{name:<16} {slot:>4}");
    offset + 2
}

fn jump_instruction(
    out: &mut String,
    name: &str,
    sign: i32,
    chunk: &Chunk,
    offset: usize,
) -> usize {
    use std::fmt::Write;
    let hi = chunk.code[offset + 1] as u16;
    let lo = chunk.code[offset + 2] as u16;
    let jump = (hi << 8) | lo;
    let target = (offset + 3) as i32 + sign * jump as i32;
    let _ = writeln!(out, "{name:<16} {offset:>4} -> {target}");
    offset + 3
}

fn simple_instruction(out: &mut String, name: &str, offset: usize) -> usize {
    out.push_str(name);
    out.push('\n');
    offset + 1
}

fn constant_instruction(out: &mut String, name: &str, chunk: &Chunk, offset: usize) -> usize {
    use std::fmt::Write;
    let idx = chunk.code[offset + 1];
    let value = chunk
        .constants
        .get(idx as usize)
        .map(Value::display)
        .unwrap_or_else(|| "<missing>".to_string());
    let _ = writeln!(out, "{name:<16} {idx:>4} '{value}'");
    offset + 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_chunk_disassembles_to_just_a_header() {
        let chunk = Chunk::new();
        let out = disassemble(&chunk, "test");
        assert_eq!(out, "== test ==\n");
    }

    #[test]
    fn write_and_disassemble_constant_plus_return() {
        let mut chunk = Chunk::new();
        let idx = chunk.add_constant(Value::Float(1.2));
        chunk.write_op(OpCode::Constant, 1);
        chunk.write_byte(idx, 1);
        chunk.write_op(OpCode::Return, 1);
        let out = disassemble(&chunk, "demo");
        assert_eq!(
            out,
            "== demo ==\n0000    1 OP_CONSTANT         0 '1.2'\n0002    | OP_RETURN\n"
        );
    }

    #[test]
    fn line_marker_repeats_within_same_source_line() {
        let mut chunk = Chunk::new();
        chunk.write_op(OpCode::True, 5);
        chunk.write_op(OpCode::False, 5);
        chunk.write_op(OpCode::Pop, 6);
        let out = disassemble(&chunk, "lines");
        let lines: Vec<&str> = out.lines().collect();
        // First instruction at line 5 — explicit "5".
        assert!(lines[1].contains("   5 OP_TRUE"), "line 1: {}", lines[1]);
        // Second still at 5 — `|` marker.
        assert!(lines[2].contains("   | OP_FALSE"), "line 2: {}", lines[2]);
        // Third at line 6 — explicit "6".
        assert!(lines[3].contains("   6 OP_POP"), "line 3: {}", lines[3]);
    }

    #[test]
    fn opcode_round_trips_through_u8() {
        for op in [
            OpCode::Constant,
            OpCode::Nil,
            OpCode::True,
            OpCode::False,
            OpCode::Pop,
            OpCode::Add,
            OpCode::Sub,
            OpCode::Mul,
            OpCode::Div,
            OpCode::Mod,
            OpCode::Neg,
            OpCode::Not,
            OpCode::Equal,
            OpCode::NotEqual,
            OpCode::Greater,
            OpCode::GreaterEqual,
            OpCode::Less,
            OpCode::LessEqual,
            OpCode::Print,
            OpCode::Return,
            OpCode::GetLocal,
            OpCode::SetLocal,
            OpCode::JumpIfFalse,
            OpCode::JumpIfFalsePeek,
            OpCode::JumpIfTruePeek,
            OpCode::Jump,
            OpCode::Loop,
            OpCode::DefineGlobal,
            OpCode::GetGlobal,
            OpCode::SetGlobal,
            OpCode::Call,
            OpCode::BuildTuple,
            OpCode::BuildList,
            OpCode::BuildRange,
            OpCode::Index,
            OpCode::ToStr,
            OpCode::Interp,
            OpCode::In,
            OpCode::GetField,
            OpCode::Invoke,
            OpCode::ForNext,
            OpCode::SetField,
        ] {
            assert_eq!(OpCode::from_u8(op as u8), op);
        }
    }
}
