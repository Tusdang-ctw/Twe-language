//! Bytecode dispatch loop. Phase-3 sessions 7–11.
//!
//! Reads a `Chunk` produced by `crate::compiler` and evaluates it
//! against an internal value stack. Semantics match the tree-walker
//! in `crate::eval` for the subset that's compiled so far: numeric
//! arithmetic with int/float coercion, string `+` concatenation,
//! structural `==` / `!=`, numeric comparisons, control flow,
//! globals, user-defined functions with calls and returns
//! (session 9), and (session 10) heap-type literals (tuple, list,
//! range), indexing, string interpolation, `in` / `not in`, tuple
//! arithmetic, list/range built-in methods, and `for x in iter:`
//! over ranges, lists, and tuples.
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

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::{BcClassDef, BcFunction, BcInstance, Chunk, OpCode};
use crate::value::{Env, RuntimeError, Value};

/// Cap on how many times a single `every <duration>:` clock can
/// fire in one frame. Same value as `eval::MAX_CATCHUP_FIRES_PER_FRAME`
/// — eight 16ms ticks ≈ 128ms of catch-up, comfortably absorbing a
/// slow first frame while bounded so a long stall can't lock the
/// runtime in catch-up forever. Closes Phase-2 frustration F4 for
/// the bytecode VM too.
const MAX_CATCHUP_FIRES_PER_FRAME: u32 = 8;

#[derive(Copy, Clone)]
enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl ArithOp {
    fn as_str(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
        }
    }
}

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
    /// Held tree-walker env, used as a `&mut Env` slot for builtin
    /// dispatch. Pure builtins (math etc.) ignore it. The VM's own
    /// active_scene/active_entities live below — Env's are unused.
    builtin_env: Env,
    /// xorshift64* state for `range.roll`. Same algorithm and default
    /// seed as `Env` so a deterministic test seed yields the same
    /// sequence on the tree-walker and the VM.
    rng: u64,
    /// The current scene instance, set by OP_INIT_SCENE when a
    /// `scene <Name>:` declaration is compiled. `tick(dt)` ticks
    /// this scene's state machine each frame.
    active_scene: Option<Rc<RefCell<BcInstance>>>,
    /// Live entities (anything `spawn`ed). `tick(dt)` calls each
    /// entity's `.update(dt)` method (if any), then prunes any
    /// flagged despawned.
    active_entities: Vec<Rc<RefCell<BcInstance>>>,
    /// Set by `Stmt::Transition`; the scene tick consumes this
    /// after the current state-handler body completes.
    transitioning: Option<String>,
    /// Top-level `on update(dt):` handler. Fires once per
    /// `tick(dt)` before the scene tick.
    on_update: Option<Rc<BcFunction>>,
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
        // Boot a tree-walker Env so we can reuse `stdlib::install`
        // for module objects (`math`, `key`, `time`, `color`,
        // `screen`, etc.). Each binding is then mirrored into the
        // VM's own globals — Rc-shared values stay live in both
        // tables, so a builtin that mutates an Object reaches both.
        let mut env = Env::new();
        crate::stdlib::install(&mut env);
        let mut globals = HashMap::with_capacity(env.iter_bindings().count());
        for (name, value) in env.iter_bindings() {
            globals.insert(name.clone(), value.clone());
        }
        // Replace the stdlib `entities` Object (which routes through
        // tree-walker builtins) with a VM-tagged one so OP_INVOKE
        // can dispatch to our own active_entities.
        globals.insert(
            "entities".to_string(),
            Value::Object(Rc::new(RefCell::new(crate::value::Object {
                fields: HashMap::new(),
                kind: "entities",
            }))),
        );
        Self {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            globals,
            builtin_env: env,
            rng: 0x9E37_79B9_7F4A_7C15,
            active_scene: None,
            active_entities: Vec::new(),
            transitioning: None,
            on_update: None,
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
        self.dispatch(0)
    }

    /// Run the dispatch loop until the active call-frame count drops
    /// below `target_depth`. For top-level `run`, target is 0 — loop
    /// runs to completion. For VM-side nested invocations
    /// (`invoke_method_value`) target is the caller's frame count,
    /// so dispatch returns once the callee's `OP_RETURN` pops back.
    fn dispatch(&mut self, target_depth: usize) -> Result<Value, RuntimeError> {
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
                OpCode::Add => self.binary_arith(ArithOp::Add, line)?,
                OpCode::Sub => self.binary_arith(ArithOp::Sub, line)?,
                OpCode::Mul => self.binary_arith(ArithOp::Mul, line)?,
                OpCode::Div => self.binary_arith(ArithOp::Div, line)?,
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
                    self.stack.truncate(frame.slot_base);
                    if self.frames.len() < target_depth {
                        // Nested invocation completing: leave the
                        // result on the stack so the VM-side caller
                        // can pop it (mirrors the OP_CALL / OP_INVOKE
                        // contract for bytecode callers).
                        self.push(result.clone());
                        return Ok(result);
                    }
                    if self.frames.is_empty() {
                        // Script frame ended at top level. The result
                        // is the script's return value (typically Nil).
                        return Ok(result);
                    }
                    // Bytecode-internal call returning to a caller
                    // frame: push the return value for the caller.
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
                OpCode::BuildTuple => {
                    let n = self.read_byte() as usize;
                    let elems = self.pop_n(n, line)?;
                    self.push(Value::Tuple(Rc::new(elems)));
                }
                OpCode::BuildList => {
                    let n = self.read_byte() as usize;
                    let elems = self.pop_n(n, line)?;
                    self.push(Value::List(Rc::new(RefCell::new(elems))));
                }
                OpCode::BuildRange => {
                    let exclusive = self.read_byte() != 0;
                    let end = self.pop()?;
                    let start = self.pop()?;
                    match (&start, &end) {
                        (Value::Int(a), Value::Int(b)) => self.push(Value::Range {
                            start: *a,
                            end: *b,
                            exclusive,
                        }),
                        _ => {
                            return Err(RuntimeError {
                                line,
                                col: 0,
                                message: format!(
                                    "range bounds must be ints, got {} and {}",
                                    start.type_name(),
                                    end.type_name()
                                ),
                                help: Some(
                                    "v0.1 supports only integer ranges; float ranges ship later"
                                        .to_string(),
                                ),
                            });
                        }
                    }
                }
                OpCode::Index => {
                    let idx = self.pop()?;
                    let obj = self.pop()?;
                    let v = index_get(&obj, &idx, line)?;
                    self.push(v);
                }
                OpCode::ToStr => {
                    let v = self.pop()?;
                    self.push(Value::Str(Rc::new(v.display())));
                }
                OpCode::Interp => {
                    let n = self.read_byte() as usize;
                    let parts = self.pop_n(n, line)?;
                    let mut out = String::new();
                    for p in &parts {
                        match p {
                            Value::Str(s) => out.push_str(s),
                            other => {
                                return Err(RuntimeError {
                                    line,
                                    col: 0,
                                    message: format!(
                                        "vm: OP_INTERP got non-string part {}",
                                        other.type_name()
                                    ),
                                    help: Some(
                                        "compiler bug — every interp part should be a Str \
                                         (text constants and OP_TO_STR-coerced exprs)"
                                            .to_string(),
                                    ),
                                });
                            }
                        }
                    }
                    self.push(Value::Str(Rc::new(out)));
                }
                OpCode::In => {
                    let haystack = self.pop()?;
                    let needle = self.pop()?;
                    let found = value_in(&needle, &haystack, line)?;
                    self.push(Value::Bool(found));
                }
                OpCode::GetField => {
                    let idx = self.read_byte() as usize;
                    let name = self.read_string_constant(idx, line)?;
                    let recv = self.pop()?;
                    let v = field_get(&recv, &name, line)?;
                    self.push(v);
                }
                OpCode::SetField => {
                    let idx = self.read_byte() as usize;
                    let name = self.read_string_constant(idx, line)?;
                    // Stack: [..., recv, value]. Set leaves the value
                    // on top so the assignment expression's caller can
                    // OP_POP it (statement context) or use it (rare).
                    let value = self.pop()?;
                    let recv = self.pop()?;
                    field_set(&recv, &name, value.clone(), line)?;
                    self.push(value);
                }
                OpCode::InitScene => {
                    let class_val = self.pop()?;
                    let class = match class_val {
                        Value::BcClass(c) => c,
                        other => {
                            return Err(RuntimeError {
                                line,
                                col: 0,
                                message: format!(
                                    "OP_INIT_SCENE expected a class, got {}",
                                    other.type_name()
                                ),
                                help: Some("compiler bug".to_string()),
                            });
                        }
                    };
                    let inst = match instantiate_bc(class.clone()) {
                        Value::BcInstance(rc) => rc,
                        _ => unreachable!(),
                    };
                    self.active_scene = Some(inst.clone());
                    if let Some(start) = class.initial_state.clone() {
                        self.enter_state(&inst, &start)?;
                    }
                }
                OpCode::Spawn => {
                    let with_at = self.read_byte() != 0;
                    let class_val = self.pop()?;
                    let at_value = if with_at { Some(self.pop()?) } else { None };
                    let class = match class_val {
                        Value::BcClass(c) => c,
                        other => {
                            return Err(RuntimeError {
                                line,
                                col: 0,
                                message: format!(
                                    "`spawn` expected a class, got {}",
                                    other.type_name()
                                ),
                                help: None,
                            });
                        }
                    };
                    let inst = match instantiate_bc(class.clone()) {
                        Value::BcInstance(rc) => rc,
                        _ => unreachable!(),
                    };
                    if let Some(at) = at_value {
                        inst.borrow_mut().fields.insert("pos".to_string(), at);
                    }
                    self.active_entities.push(inst.clone());
                    if let Some(start) = class.initial_state.clone() {
                        self.enter_state(&inst, &start)?;
                    }
                }
                OpCode::Despawn => {
                    let target = self.pop()?;
                    match target {
                        Value::BcInstance(rc) => {
                            rc.borrow_mut().despawned = true;
                        }
                        other => {
                            return Err(RuntimeError {
                                line,
                                col: 0,
                                message: format!(
                                    "`despawn` expects an instance, got {}",
                                    other.type_name()
                                ),
                                help: None,
                            });
                        }
                    }
                }
                OpCode::Transition => {
                    let idx = self.read_byte() as usize;
                    let target = self.read_string_constant(idx, line)?;
                    self.transitioning = Some(target);
                }
                OpCode::SetOnUpdate => {
                    let idx = self.read_byte() as usize;
                    let value = self.read_constant(idx);
                    match value {
                        Value::BcFunction(func) => {
                            self.on_update = Some(func);
                        }
                        other => {
                            return Err(RuntimeError {
                                line,
                                col: 0,
                                message: format!(
                                    "OP_SET_ON_UPDATE expected a function, got {}",
                                    other.type_name()
                                ),
                                help: Some("compiler bug".to_string()),
                            });
                        }
                    }
                }
                OpCode::Invoke => {
                    let name_idx = self.read_byte() as usize;
                    let arg_count = self.read_byte() as usize;
                    let name = self.read_string_constant(name_idx, line)?;
                    self.invoke_method(&name, arg_count, line)?;
                }
                OpCode::ForNext => {
                    let base_slot = self.read_byte() as usize;
                    let exit_offset = self.read_u16();
                    let abs_iter = self.frames.last().unwrap().slot_base + base_slot;
                    let abs_counter = abs_iter + 1;
                    let counter = match self.stack.get(abs_counter) {
                        Some(Value::Int(n)) => *n,
                        other => {
                            return Err(RuntimeError {
                                line,
                                col: 0,
                                message: format!(
                                    "vm: for-loop counter not an int (got {})",
                                    other.map(Value::type_name).unwrap_or("missing")
                                ),
                                help: Some("compiler bug".to_string()),
                            });
                        }
                    };
                    let iter_value = self.stack[abs_iter].clone();
                    let next = match &iter_value {
                        Value::Range { start, end, exclusive } => {
                            let limit = if *exclusive { *end } else { *end + 1 };
                            let cur = *start + counter;
                            if cur < limit {
                                Some(Value::Int(cur))
                            } else {
                                None
                            }
                        }
                        Value::List(rc) => {
                            let v = rc.borrow();
                            if (counter as usize) < v.len() {
                                Some(v[counter as usize].clone())
                            } else {
                                None
                            }
                        }
                        Value::Tuple(elems) => {
                            if (counter as usize) < elems.len() {
                                Some(elems[counter as usize].clone())
                            } else {
                                None
                            }
                        }
                        other => {
                            return Err(RuntimeError {
                                line,
                                col: 0,
                                message: format!(
                                    "for-loop iterable must be a range, list, or tuple, \
                                     got {}",
                                    other.type_name()
                                ),
                                help: None,
                            });
                        }
                    };
                    match next {
                        Some(elem) => {
                            self.stack[abs_counter] = Value::Int(counter + 1);
                            self.push(elem);
                        }
                        None => {
                            self.frames.last_mut().unwrap().ip += exit_offset as usize;
                        }
                    }
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
            Value::BcFunction(func) => self.push_call_frame(func, callee_idx, arg_count, line),
            Value::BcClass(class) => {
                if arg_count != 0 {
                    return Err(RuntimeError {
                        line,
                        col: 0,
                        message: format!(
                            "constructor for {} takes no arguments yet (got {})",
                            class.name, arg_count
                        ),
                        help: None,
                    });
                }
                let inst = instantiate_bc(class);
                // Pop the class value and replace with the instance.
                self.stack.truncate(callee_idx);
                self.push(inst);
                Ok(())
            }
            Value::Builtin { name, params, func } => {
                let args: Vec<Value> = self.stack.drain(callee_idx + 1..).collect();
                if !params.is_empty() && args.len() != params.len() {
                    return Err(RuntimeError {
                        line,
                        col: 0,
                        message: format!(
                            "builtin `{name}` expected {} arguments, got {}",
                            params.len(),
                            args.len()
                        ),
                        help: None,
                    });
                }
                let result = func(&mut self.builtin_env, &args)?;
                // Pop the builtin value, push result.
                self.stack.pop();
                self.push(result);
                Ok(())
            }
            other => Err(RuntimeError {
                line,
                col: 0,
                message: format!(
                    "tried to call a {} (only functions and classes are callable)",
                    other.type_name()
                ),
                help: None,
            }),
        }
    }

    /// Drive one frame of the play loop. Order mirrors
    /// `eval::tick_frame`:
    ///   1. update `time.dt` ambient
    ///   2. fire top-level `on update(dt):` if any
    ///   3. tick the active scene's state machine (state on_update,
    ///      then every-clocks with bounded catch-up)
    ///   4. tick each active entity's `update(dt)` method
    ///   5. prune entities flagged despawned
    pub fn tick(&mut self, dt: f64) -> Result<(), RuntimeError> {
        self.update_time_dt(dt);
        if let Some(handler) = self.on_update.clone() {
            // Top-level on_update has no `self`; we still pass the
            // function value as slot 0 (per the BcFunction calling
            // convention) and `dt` as slot 1.
            let dummy_recv = Value::BcFunction(handler.clone());
            self.invoke_method_value(handler, dummy_recv, &[Value::Float(dt)])?;
        }
        if let Some(scene) = self.active_scene.clone() {
            self.tick_scene(&scene, dt)?;
        }
        let entities = self.active_entities.clone();
        for entity in entities {
            if entity.borrow().despawned {
                continue;
            }
            let class = entity.borrow().class.clone();
            if let Some(method) = class.methods.get("update").cloned() {
                self.invoke_method_value(
                    method,
                    Value::BcInstance(entity.clone()),
                    &[Value::Float(dt)],
                )?;
            }
        }
        self.active_entities.retain(|e| !e.borrow().despawned);
        Ok(())
    }

    fn tick_scene(
        &mut self,
        scene: &Rc<RefCell<BcInstance>>,
        dt: f64,
    ) -> Result<(), RuntimeError> {
        // Snapshot the current state's bodies before running so a
        // transition mid-tick doesn't fire the new state's clocks
        // this frame (matches `eval::tick_scene`).
        let (state_on_update, clocks) = {
            let inst = scene.borrow();
            let state = match inst
                .current_state
                .as_ref()
                .and_then(|n| inst.class.states.get(n))
            {
                Some(s) => s.clone(),
                None => return Ok(()),
            };
            (state.on_update.clone(), state.every_clocks.clone())
        };

        // State-scoped `on update(dt):` fires once per frame BEFORE
        // every-clocks. A transition inside it skips the clocks for
        // this frame; we re-enter the new state immediately.
        if let Some(handler) = state_on_update {
            self.invoke_method_value(
                handler,
                Value::BcInstance(scene.clone()),
                &[Value::Float(dt)],
            )?;
            if let Some(target) = self.transitioning.take() {
                return self.enter_state(scene, &target);
            }
        }

        // Tick each clock with bounded catch-up. If a transition
        // fires inside a clock body, we exit the loop and re-enter
        // immediately (no further clocks fire this frame).
        let mut transition: Option<String> = None;
        'clocks: for (clock_idx, (interval, body)) in clocks.into_iter().enumerate() {
            {
                let mut inst = scene.borrow_mut();
                if clock_idx >= inst.every_timers.len() {
                    continue;
                }
                inst.every_timers[clock_idx] += dt;
            }
            let mut fires: u32 = 0;
            while fires < MAX_CATCHUP_FIRES_PER_FRAME {
                let should_fire = scene
                    .borrow()
                    .every_timers
                    .get(clock_idx)
                    .copied()
                    .unwrap_or(0.0)
                    >= interval;
                if !should_fire {
                    break;
                }
                scene.borrow_mut().every_timers[clock_idx] -= interval;
                fires += 1;
                self.invoke_method_value(
                    body.clone(),
                    Value::BcInstance(scene.clone()),
                    &[],
                )?;
                if let Some(target) = self.transitioning.take() {
                    transition = Some(target);
                    break 'clocks;
                }
            }
            if fires >= MAX_CATCHUP_FIRES_PER_FRAME {
                // Drop residual so next frame starts fresh and doesn't
                // compound the backlog.
                scene.borrow_mut().every_timers[clock_idx] = 0.0;
            }
        }
        if let Some(target) = transition {
            self.enter_state(scene, &target)?;
        }
        Ok(())
    }

    fn enter_state(
        &mut self,
        scene: &Rc<RefCell<BcInstance>>,
        state_name: &str,
    ) -> Result<(), RuntimeError> {
        let state = scene
            .borrow()
            .class
            .states
            .get(state_name)
            .cloned()
            .ok_or_else(|| RuntimeError {
                line: 0,
                col: 0,
                message: format!("no state named '{state_name}'"),
                help: Some(
                    "transitions must target a `state <name>:` declared in the same scene"
                        .to_string(),
                ),
            })?;
        {
            let mut inst = scene.borrow_mut();
            inst.current_state = Some(state.name.clone());
            inst.every_timers = vec![0.0; state.every_clocks.len()];
            inst.every_intervals_secs = state.every_clocks.iter().map(|(s, _)| *s).collect();
        }
        // Run on_entry. A transition inside the body cascades.
        self.invoke_method_value(
            state.on_entry.clone(),
            Value::BcInstance(scene.clone()),
            &[],
        )?;
        if let Some(next) = self.transitioning.take() {
            return self.enter_state(scene, &next);
        }
        Ok(())
    }

    /// VM-side dispatch for `entities.of(Class)` / `entities.count(Class)`.
    /// The held `builtin_env`'s entities builtins look at the tree-
    /// walker's active_entities and would always return empty for
    /// us — so we intercept here. When the BuiltinFn signature gets
    /// unified across both interpreters this becomes redundant.
    fn entities_intrinsic(
        &mut self,
        name: &str,
        args: &[Value],
        line: u32,
    ) -> Result<Value, RuntimeError> {
        let class = match args.first() {
            Some(Value::BcClass(c)) => c.clone(),
            Some(other) => {
                return Err(RuntimeError {
                    line,
                    col: 0,
                    message: format!(
                        "entities.{name} expected a class, got {}",
                        other.type_name()
                    ),
                    help: None,
                });
            }
            None => {
                return Err(RuntimeError {
                    line,
                    col: 0,
                    message: format!("entities.{name} expected 1 argument, got 0"),
                    help: None,
                });
            }
        };
        if args.len() != 1 {
            return Err(RuntimeError {
                line,
                col: 0,
                message: format!(
                    "entities.{name} expected 1 argument, got {}",
                    args.len()
                ),
                help: None,
            });
        }
        let matches: Vec<Rc<RefCell<BcInstance>>> = self
            .active_entities
            .iter()
            .filter(|e| {
                !e.borrow().despawned && Rc::ptr_eq(&e.borrow().class, &class)
            })
            .cloned()
            .collect();
        match name {
            "count" => Ok(Value::Int(matches.len() as i64)),
            "of" => {
                let list: Vec<Value> = matches.into_iter().map(Value::BcInstance).collect();
                Ok(Value::List(Rc::new(RefCell::new(list))))
            }
            _ => Err(RuntimeError {
                line,
                col: 0,
                message: format!("entities has no method '{name}'"),
                help: Some("entities methods are .of(Class), .count(Class)".to_string()),
            }),
        }
    }

    /// Update `time.dt` on the global `time` Object so scene/entity
    /// code can read it as an ambient. Mirrors `eval::update_time_ambient`.
    fn update_time_dt(&mut self, dt: f64) {
        if let Some(Value::Object(rc)) = self.globals.get("time") {
            rc.borrow_mut()
                .fields
                .insert("dt".to_string(), Value::Float(dt));
        }
    }

    /// Run a `BcFunction` with `recv` as slot 0 and `args` as
    /// slots 1..=arity. Used by VM-side machinery (state-machine
    /// tick, entity update, top-level on_update) to invoke
    /// compiled bodies. Returns the value the body popped at its
    /// `OP_RETURN`; the stack is clean afterwards (the result is
    /// also popped before return so callers don't have to).
    fn invoke_method_value(
        &mut self,
        function: Rc<BcFunction>,
        recv: Value,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        let recv_idx = self.stack.len();
        self.stack.push(recv);
        for arg in args {
            self.stack.push(arg.clone());
        }
        let target_depth = self.frames.len();
        self.push_call_frame(function, recv_idx, args.len(), 0)?;
        let result = self.dispatch(target_depth + 1)?;
        // dispatch() left the result on top; pop to keep the stack clean.
        self.stack.pop();
        Ok(result)
    }

    /// Push a CallFrame for a `BcFunction`. Validates arity and the
    /// frame-stack bound. The function value at `callee_idx` becomes
    /// the new frame's slot 0; args sit at slots 1..=arity.
    fn push_call_frame(
        &mut self,
        func: Rc<BcFunction>,
        callee_idx: usize,
        arg_count: usize,
        line: u32,
    ) -> Result<(), RuntimeError> {
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
                    "call stack exceeded {FRAMES_MAX} frames — likely unbounded recursion"
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

    /// Pop the top `n` values, returning them in source order (the
    /// value pushed first is index 0). Used by OP_BUILD_TUPLE,
    /// OP_BUILD_LIST, OP_INTERP — all of which need the constructor
    /// values in the order they were emitted.
    fn pop_n(&mut self, n: usize, line: u32) -> Result<Vec<Value>, RuntimeError> {
        if self.stack.len() < n {
            return Err(RuntimeError {
                line,
                col: 0,
                message: format!(
                    "vm: stack underflow popping {n} values (have {})",
                    self.stack.len()
                ),
                help: Some("compiler bug".to_string()),
            });
        }
        let at = self.stack.len() - n;
        Ok(self.stack.drain(at..).collect())
    }

    /// OP_INVOKE handler. The receiver is on the stack at `top -
    /// arg_count`, with the args above it. Dispatches by receiver
    /// type to built-in methods; mirrors `eval::list_method_call` /
    /// `eval::range_method_call`. User-defined methods on Instance
    /// land with the declarative-block pass in session 11.
    fn invoke_method(
        &mut self,
        name: &str,
        arg_count: usize,
        line: u32,
    ) -> Result<(), RuntimeError> {
        let recv_idx = self
            .stack
            .len()
            .checked_sub(arg_count + 1)
            .ok_or_else(|| RuntimeError {
                line,
                col: 0,
                message: format!(
                    "vm: stack underflow on Invoke (arg_count={arg_count}, stack={})",
                    self.stack.len()
                ),
                help: None,
            })?;
        // BcInstance dispatch keeps the receiver on the stack as the
        // method's slot 0 (`self`) and continues from the new frame —
        // it doesn't drop into the simple "compute one value" pattern.
        let recv_clone = self.stack[recv_idx].clone();
        if let Value::BcInstance(inst_rc) = &recv_clone {
            let method = inst_rc
                .borrow()
                .class
                .methods
                .get(name)
                .cloned()
                .ok_or_else(|| RuntimeError {
                    line,
                    col: 0,
                    message: format!(
                        "method `.{name}` is not defined on instance of {}",
                        inst_rc.borrow().class.name
                    ),
                    help: None,
                })?;
            return self.push_call_frame(method, recv_idx, arg_count, line);
        }
        // VM-side intrinsics for the `entities` module: of(Class)
        // and count(Class) read from `self.active_entities`, which
        // the held builtin_env doesn't see. The tree-walker's
        // entities.of/count Builtins look at `Env::active_entities`;
        // for the bytecode VM we route to BcInstance values here.
        if let Value::Object(rc) = &recv_clone {
            if rc.borrow().kind == "entities" {
                let args: Vec<Value> = self.stack.drain(recv_idx + 1..).collect();
                self.stack.pop(); // drop receiver
                let result = self.entities_intrinsic(name, &args, line)?;
                self.push(result);
                return Ok(());
            }
        }
        // Object module access: `math.min(...)`. The "method" is
        // really a Builtin field; look it up and call it with args.
        if let Value::Object(rc) = &recv_clone {
            let field = rc.borrow().fields.get(name).cloned().ok_or_else(|| {
                RuntimeError {
                    line,
                    col: 0,
                    message: format!("module `{}` has no field '{name}'", rc.borrow().kind),
                    help: None,
                }
            })?;
            let args: Vec<Value> = self.stack.drain(recv_idx + 1..).collect();
            self.stack.pop(); // drop the receiver
            let result = match field {
                Value::Builtin { name: bname, params, func } => {
                    if !params.is_empty() && args.len() != params.len() {
                        return Err(RuntimeError {
                            line,
                            col: 0,
                            message: format!(
                                "builtin `{bname}` expected {} arguments, got {}",
                                params.len(),
                                args.len()
                            ),
                            help: None,
                        });
                    }
                    func(&mut self.builtin_env, &args)?
                }
                other => {
                    return Err(RuntimeError {
                        line,
                        col: 0,
                        message: format!(
                            "field `.{name}` on module is a {}, not callable",
                            other.type_name()
                        ),
                        help: None,
                    });
                }
            };
            self.push(result);
            return Ok(());
        }
        // Built-in receivers: list / range methods (session 10).
        let args: Vec<Value> = self.stack.drain(recv_idx + 1..).collect();
        let recv = self.stack.pop().expect("receiver");
        let result = match &recv {
            Value::List(rc) => list_method(rc, name, &args, line)?,
            Value::Range { start, end, exclusive } => {
                range_method(*start, *end, *exclusive, name, &args, line, &mut self.rng)?
            }
            other => {
                return Err(RuntimeError {
                    line,
                    col: 0,
                    message: format!(
                        "method `.{name}` is not defined on {}",
                        other.type_name()
                    ),
                    help: None,
                });
            }
        };
        self.push(result);
        Ok(())
    }

    fn binary_arith(&mut self, op: ArithOp, line: u32) -> Result<(), RuntimeError> {
        let r = self.pop()?;
        let l = self.pop()?;
        let result = apply_arith(op, &l, &r, line)?;
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
    // Mirror `eval::values_equal`. Tuple equality recurses; Range
    // and Str compare by structural identity. Lists are
    // intentionally compared by Rc identity (same as eval) — two
    // distinct list values with equal contents are not `==`,
    // matching the mutability story.
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
        (
            Value::Range { start: s1, end: e1, exclusive: x1 },
            Value::Range { start: s2, end: e2, exclusive: x2 },
        ) => s1 == s2 && e1 == e2 && x1 == x2,
        _ => false,
    }
}

/// Numeric / string / tuple arithmetic. Mirrors `eval::apply_arith`
/// but takes a small `ArithOp` enum so the VM dispatch can stay
/// flat. Tuple element-wise + / - and tuple <-> scalar * / / are
/// here so Snake-style `cell * cell_size` and `pos + direction`
/// produce the same Tuple values as the tree-walker.
fn apply_arith(op: ArithOp, l: &Value, r: &Value, line: u32) -> Result<Value, RuntimeError> {
    // String concatenation via `+`.
    if matches!(op, ArithOp::Add) {
        if let (Value::Str(a), Value::Str(b)) = (l, r) {
            let mut s = String::with_capacity(a.len() + b.len());
            s.push_str(a);
            s.push_str(b);
            return Ok(Value::Str(Rc::new(s)));
        }
    }
    // Tuple element-wise + / -.
    if let (Value::Tuple(a), Value::Tuple(b)) = (l, r) {
        if matches!(op, ArithOp::Add | ArithOp::Sub) {
            if a.len() != b.len() {
                return Err(RuntimeError {
                    line,
                    col: 0,
                    message: format!(
                        "tuple {} requires equal-length operands ({} vs {})",
                        op.as_str(),
                        a.len(),
                        b.len()
                    ),
                    help: None,
                });
            }
            let mut out = Vec::with_capacity(a.len());
            for (x, y) in a.iter().zip(b.iter()) {
                out.push(apply_arith(op, x, y, line)?);
            }
            return Ok(Value::Tuple(Rc::new(out)));
        }
    }
    // Tuple * / / scalar.
    if let Value::Tuple(elems) = l {
        if matches!(op, ArithOp::Mul | ArithOp::Div) && is_scalar(r) {
            let mut out = Vec::with_capacity(elems.len());
            for x in elems.iter() {
                out.push(apply_arith(op, x, r, line)?);
            }
            return Ok(Value::Tuple(Rc::new(out)));
        }
    }
    // scalar * Tuple.
    if let Value::Tuple(elems) = r {
        if matches!(op, ArithOp::Mul) && is_scalar(l) {
            let mut out = Vec::with_capacity(elems.len());
            for y in elems.iter() {
                out.push(apply_arith(op, l, y, line)?);
            }
            return Ok(Value::Tuple(Rc::new(out)));
        }
    }
    // Scalar paths.
    let result = match (op, l, r) {
        (ArithOp::Div, Value::Int(_), Value::Int(0)) => return Err(division_by_zero(line)),
        (ArithOp::Add, Value::Int(a), Value::Int(b)) => Value::Int(a + b),
        (ArithOp::Sub, Value::Int(a), Value::Int(b)) => Value::Int(a - b),
        (ArithOp::Mul, Value::Int(a), Value::Int(b)) => Value::Int(a * b),
        (ArithOp::Div, Value::Int(a), Value::Int(b)) => Value::Int(a / b),
        (ArithOp::Add, Value::Float(a), Value::Float(b)) => Value::Float(a + b),
        (ArithOp::Sub, Value::Float(a), Value::Float(b)) => Value::Float(a - b),
        (ArithOp::Mul, Value::Float(a), Value::Float(b)) => Value::Float(a * b),
        (ArithOp::Div, Value::Float(a), Value::Float(b)) => Value::Float(a / b),
        (op, Value::Int(a), Value::Float(b)) => mix_float(op, *a as f64, *b, line)?,
        (op, Value::Float(a), Value::Int(b)) => mix_float(op, *a, *b as f64, line)?,
        _ => return Err(type_error(op.as_str(), l, r, line)),
    };
    Ok(result)
}

fn mix_float(op: ArithOp, a: f64, b: f64, line: u32) -> Result<Value, RuntimeError> {
    Ok(match op {
        ArithOp::Add => Value::Float(a + b),
        ArithOp::Sub => Value::Float(a - b),
        ArithOp::Mul => Value::Float(a * b),
        ArithOp::Div => {
            if b == 0.0 {
                return Err(division_by_zero(line));
            }
            Value::Float(a / b)
        }
    })
}

fn is_scalar(v: &Value) -> bool {
    matches!(v, Value::Int(_) | Value::Float(_))
}

/// Mirrors `eval::index_get`. Lists and tuples are 0-indexed;
/// negative indices count from the end (Principle 3).
fn index_get(obj: &Value, idx: &Value, line: u32) -> Result<Value, RuntimeError> {
    match (obj, idx) {
        (Value::List(rc), Value::Int(i)) => {
            let v = rc.borrow();
            let len = v.len() as i64;
            let actual = if *i < 0 { *i + len } else { *i };
            if actual < 0 || actual >= len {
                return Err(RuntimeError {
                    line,
                    col: 0,
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
                    col: 0,
                    message: format!("tuple index {i} out of bounds (length {len})"),
                    help: None,
                });
            }
            Ok(elems[actual as usize].clone())
        }
        (Value::List(_) | Value::Tuple(_), other) => Err(RuntimeError {
            line,
            col: 0,
            message: format!("index must be int, got {}", other.type_name()),
            help: None,
        }),
        (other, _) => Err(RuntimeError {
            line,
            col: 0,
            message: format!("cannot index value of type {}", other.type_name()),
            help: Some("indexing works on lists and tuples".to_string()),
        }),
    }
}

/// Mirrors `eval::field_get` for the subset the bytecode VM
/// reaches: tuples, lists, BcInstances, and Objects (the latter
/// covers module builtins like `math`, `time`, `key`).
fn field_get(obj: &Value, name: &str, line: u32) -> Result<Value, RuntimeError> {
    match obj {
        Value::Tuple(elems) => match name {
            "x" if !elems.is_empty() => Ok(elems[0].clone()),
            "y" if elems.len() >= 2 => Ok(elems[1].clone()),
            "z" if elems.len() >= 3 => Ok(elems[2].clone()),
            _ => Err(RuntimeError {
                line,
                col: 0,
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
                col: 0,
                message: format!("list has no field '{name}'"),
                help: Some(
                    "lists expose .length; methods are .append, .prepend, .pop_back, \
                     .pop_front, .contains"
                        .to_string(),
                ),
            }),
        },
        Value::BcInstance(rc) => {
            let inst = rc.borrow();
            inst.fields.get(name).cloned().ok_or_else(|| RuntimeError {
                line,
                col: 0,
                message: format!(
                    "field '{name}' is not defined on instance of {}",
                    inst.class.name
                ),
                help: None,
            })
        }
        Value::Object(rc) => rc.borrow().fields.get(name).cloned().ok_or_else(|| {
            RuntimeError {
                line,
                col: 0,
                message: format!("module `{}` has no field '{name}'", rc.borrow().kind),
                help: None,
            }
        }),
        other => Err(RuntimeError {
            line,
            col: 0,
            message: format!(
                "cannot read field '.{name}' on a {}",
                other.type_name()
            ),
            help: None,
        }),
    }
}

/// `recv.name = value`. BcInstance stores in its fields HashMap;
/// Object likewise. Other receivers error.
fn field_set(recv: &Value, name: &str, value: Value, line: u32) -> Result<(), RuntimeError> {
    match recv {
        Value::BcInstance(rc) => {
            rc.borrow_mut().fields.insert(name.to_string(), value);
            Ok(())
        }
        Value::Object(rc) => {
            rc.borrow_mut().fields.insert(name.to_string(), value);
            Ok(())
        }
        other => Err(RuntimeError {
            line,
            col: 0,
            message: format!(
                "cannot assign field '.{name}' on a {}",
                other.type_name()
            ),
            help: None,
        }),
    }
}

/// Walk the class's defaults to materialise a fresh instance.
/// Defaults are cloned so each instance gets its own copy
/// (Rc-shared values like Lists still share their interior, which
/// matches the tree-walker semantics). State-machine fields stay
/// empty; `enter_state` populates them when the scene boots.
fn instantiate_bc(class: Rc<BcClassDef>) -> Value {
    let fields: HashMap<String, Value> = class
        .field_defaults
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Value::BcInstance(Rc::new(RefCell::new(BcInstance {
        class,
        fields,
        current_state: None,
        every_timers: Vec::new(),
        every_intervals_secs: Vec::new(),
        despawned: false,
    })))
}

/// Mirrors `eval::value_in`. List/Tuple/Range/Str membership.
fn value_in(needle: &Value, haystack: &Value, line: u32) -> Result<bool, RuntimeError> {
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
            col: 0,
            message: format!(
                "`in` expects a list, tuple, range, or string, got {}",
                other.type_name()
            ),
            help: None,
        }),
    }
}

fn list_method(
    rc: &Rc<RefCell<Vec<Value>>>,
    name: &str,
    args: &[Value],
    line: u32,
) -> Result<Value, RuntimeError> {
    let arity_check = |expected: usize| -> Result<(), RuntimeError> {
        if args.len() != expected {
            Err(RuntimeError {
                line,
                col: 0,
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
            rc.borrow_mut().push(args[0].clone());
            Ok(Value::Nil)
        }
        "prepend" => {
            arity_check(1)?;
            rc.borrow_mut().insert(0, args[0].clone());
            Ok(Value::Nil)
        }
        "pop_back" => {
            arity_check(0)?;
            rc.borrow_mut().pop().ok_or_else(|| RuntimeError {
                line,
                col: 0,
                message: "pop_back on an empty list".to_string(),
                help: Some("guard with `if list.length > 0:` before popping".to_string()),
            })
        }
        "pop_front" => {
            arity_check(0)?;
            let mut v = rc.borrow_mut();
            if v.is_empty() {
                return Err(RuntimeError {
                    line,
                    col: 0,
                    message: "pop_front on an empty list".to_string(),
                    help: Some("guard with `if list.length > 0:` before popping".to_string()),
                });
            }
            Ok(v.remove(0))
        }
        "contains" => {
            arity_check(1)?;
            let found = rc.borrow().iter().any(|v| values_equal(v, &args[0]));
            Ok(Value::Bool(found))
        }
        _ => Err(RuntimeError {
            line,
            col: 0,
            message: format!("list has no method '{name}'"),
            help: Some(
                "list methods are .append, .prepend, .pop_back, .pop_front, .contains".to_string(),
            ),
        }),
    }
}

fn range_method(
    start: i64,
    end: i64,
    exclusive: bool,
    name: &str,
    args: &[Value],
    line: u32,
    rng: &mut u64,
) -> Result<Value, RuntimeError> {
    match name {
        "roll" => {
            if !args.is_empty() {
                return Err(RuntimeError {
                    line,
                    col: 0,
                    message: format!("range.roll expected 0 arguments, got {}", args.len()),
                    help: None,
                });
            }
            let upper = if exclusive { end } else { end + 1 };
            if upper <= start {
                return Err(RuntimeError {
                    line,
                    col: 0,
                    message: "range.roll on an empty range".to_string(),
                    help: None,
                });
            }
            let n = next_random_u64(rng);
            let span = (upper - start) as u64;
            Ok(Value::Int(start + (n % span) as i64))
        }
        "contains" => {
            if args.len() != 1 {
                return Err(RuntimeError {
                    line,
                    col: 0,
                    message: "range.contains expected 1 argument".to_string(),
                    help: None,
                });
            }
            let upper = if exclusive { end } else { end + 1 };
            let result = match &args[0] {
                Value::Int(n) => *n >= start && *n < upper,
                _ => false,
            };
            Ok(Value::Bool(result))
        }
        _ => Err(RuntimeError {
            line,
            col: 0,
            message: format!("range has no method '{name}'"),
            help: Some("range methods are .roll, .contains".to_string()),
        }),
    }
}

/// xorshift64* PRNG; same algorithm as `Env::next_random_u64` so a
/// fixed seed produces the same sequence on both interpreters.
fn next_random_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
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

    // --- Session 10: heap types + for-loops ---

    #[test]
    fn vm_builds_tuple_and_indexes_it() {
        let out = run_program("let p = (3, 4, 5)\nprint(p[0])\nprint(p[1])\nprint(p[2])").expect("ok");
        assert_eq!(out, "3\n4\n5\n");
    }

    #[test]
    fn vm_tuple_field_xyz() {
        let out = run_program("let p = (10, 20, 30)\nprint(p.x)\nprint(p.y)\nprint(p.z)").expect("ok");
        assert_eq!(out, "10\n20\n30\n");
    }

    #[test]
    fn vm_tuple_arithmetic_elementwise() {
        // Snake's `snake[0] + direction` shape.
        let out = run_program("let pos = (3, 4)\nlet dir = (1, 0)\nprint(pos + dir)").expect("ok");
        assert_eq!(out, "(4, 4)\n");
    }

    #[test]
    fn vm_tuple_times_scalar() {
        // Snake's `cell * cell_size` shape.
        let out = run_program("let cell = (2, 3)\nprint(cell * 10)").expect("ok");
        assert_eq!(out, "(20, 30)\n");
    }

    #[test]
    fn vm_scalar_times_tuple() {
        let out = run_program("let cell = (2, 3)\nprint(4 * cell)").expect("ok");
        assert_eq!(out, "(8, 12)\n");
    }

    #[test]
    fn vm_tuple_equality_recurses() {
        let out = run_program("print((1, 2) == (1, 2))\nprint((1, 2) == (1, 3))").expect("ok");
        assert_eq!(out, "true\nfalse\n");
    }

    #[test]
    fn vm_builds_list_and_indexes() {
        let out = run_program("let xs = [10, 20, 30]\nprint(xs[0])\nprint(xs[2])").expect("ok");
        assert_eq!(out, "10\n30\n");
    }

    #[test]
    fn vm_list_negative_index() {
        let out = run_program("let xs = [10, 20, 30]\nprint(xs[-1])\nprint(xs[-2])").expect("ok");
        assert_eq!(out, "30\n20\n");
    }

    #[test]
    fn vm_list_length_field() {
        let out = run_program("let xs = [1, 2, 3, 4]\nprint(xs.length)").expect("ok");
        assert_eq!(out, "4\n");
    }

    #[test]
    fn vm_list_methods_append_pop() {
        let src = "let xs = [1, 2]\nxs.append(3)\nprint(xs.length)\nlet last = xs.pop_back()\nprint(last)\nprint(xs.length)\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "3\n3\n2\n");
    }

    #[test]
    fn vm_list_method_pop_front_and_prepend() {
        let src = "let xs = [2, 3]\nxs.prepend(1)\nprint(xs[0])\nlet first = xs.pop_front()\nprint(first)\nprint(xs.length)\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "1\n1\n2\n");
    }

    #[test]
    fn vm_list_method_contains() {
        let src = "let xs = [1, 2, 3]\nprint(xs.contains(2))\nprint(xs.contains(99))\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "true\nfalse\n");
    }

    #[test]
    fn vm_list_index_out_of_bounds_errors() {
        let err = run_program("let xs = [1, 2]\nprint(xs[5])").expect_err("should fail");
        assert!(err.message.contains("out of bounds"), "got: {}", err.message);
    }

    #[test]
    fn vm_pop_empty_list_errors() {
        let err = run_program("let xs = []\nxs.pop_back()").expect_err("should fail");
        assert!(err.message.contains("empty"), "got: {}", err.message);
    }

    #[test]
    fn vm_builds_range_inclusive_and_exclusive() {
        // `0..3` is inclusive (yields 0,1,2,3); `0..<3` is exclusive
        // (yields 0,1,2). Verify via for-loop iteration counts.
        let out = run_program("let total = 0\nfor i in 0..3:\n    total = total + 1\nprint(total)").expect("ok");
        assert_eq!(out, "4\n");
        let out = run_program("let total = 0\nfor i in 0..<3:\n    total = total + 1\nprint(total)").expect("ok");
        assert_eq!(out, "3\n");
    }

    #[test]
    fn vm_for_over_range_sums_correctly() {
        // 1+2+...+10 = 55 — classic.
        let out = run_program("let sum = 0\nfor i in 1..10:\n    sum = sum + i\nprint(sum)").expect("ok");
        assert_eq!(out, "55\n");
    }

    #[test]
    fn vm_for_over_list() {
        let out = run_program("let xs = [10, 20, 30]\nfor x in xs:\n    print(x)").expect("ok");
        assert_eq!(out, "10\n20\n30\n");
    }

    #[test]
    fn vm_for_over_tuple() {
        let out = run_program("for x in (5, 6, 7):\n    print(x)").expect("ok");
        assert_eq!(out, "5\n6\n7\n");
    }

    #[test]
    fn vm_for_with_break() {
        let src = "for i in 0..10:\n    if i == 3:\n        break\n    print(i)\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "0\n1\n2\n");
    }

    #[test]
    fn vm_for_with_continue() {
        let src = "for i in 0..<5:\n    if i == 2:\n        continue\n    print(i)\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "0\n1\n3\n4\n");
    }

    #[test]
    fn vm_nested_for_loops() {
        // Each outer iteration runs the inner loop fresh; tests that
        // the unique-name hidden locals don't collide between
        // simultaneously-active for frames.
        let src = "for i in 0..<2:\n    for j in 0..<2:\n        print(i + j)\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "0\n1\n1\n2\n");
    }

    #[test]
    fn vm_range_method_contains() {
        let src = "let r = 0..10\nprint(r.contains(5))\nprint(r.contains(11))\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "true\nfalse\n");
    }

    #[test]
    fn vm_range_method_roll_is_in_range() {
        // `roll` returns an int in [start, upper). Just check
        // bounds — the seed is deterministic but we don't pin a
        // specific value because that would couple the test to the
        // RNG algorithm details.
        let src = "let r = 10..20\nlet v = r.roll()\nprint(v >= 10 and v <= 20)\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "true\n");
    }

    #[test]
    fn vm_string_interpolation_renders_values() {
        let src = "let name = \"twe\"\nlet n = 42\nprint(\"hello, {name}: {n}\")";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "hello, twe: 42\n");
    }

    #[test]
    fn vm_string_interpolation_renders_tuple_via_display() {
        let src = "let p = (3, 4)\nprint(\"pos = {p}\")";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "pos = (3, 4)\n");
    }

    #[test]
    fn vm_in_operator_on_list_and_range() {
        let out = run_program("print(2 in [1, 2, 3])").expect("ok");
        assert_eq!(out, "true\n");
        let out = run_program("print(99 in [1, 2, 3])").expect("ok");
        assert_eq!(out, "false\n");
        let out = run_program("print(5 in 0..10)").expect("ok");
        assert_eq!(out, "true\n");
        let out = run_program("print(11 in 0..10)").expect("ok");
        assert_eq!(out, "false\n");
        let out = run_program("print(11 in 0..<11)").expect("ok");
        assert_eq!(out, "false\n");
    }

    #[test]
    fn vm_not_in_operator() {
        let out = run_program("print(99 not in [1, 2, 3])").expect("ok");
        assert_eq!(out, "true\n");
    }

    #[test]
    fn vm_in_operator_substring() {
        let out = run_program("print(\"ll\" in \"hello\")").expect("ok");
        assert_eq!(out, "true\n");
        let out = run_program("print(\"xy\" in \"hello\")").expect("ok");
        assert_eq!(out, "false\n");
    }

    #[test]
    fn vm_for_over_non_iterable_errors() {
        let err = run_program("for x in 5:\n    print(x)\n").expect_err("should fail");
        assert!(
            err.message.contains("range, list, or tuple"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn vm_method_on_non_receiver_errors() {
        let err = run_program("let x = 5\nx.append(1)").expect_err("should fail");
        assert!(err.message.contains("append"), "got: {}", err.message);
    }

    #[test]
    fn vm_field_on_unsupported_receiver_errors() {
        // Field access on a number is a type error and should point
        // at where Object/Instance support lands.
        let err = run_program("let x = 5\nprint(x.foo)").expect_err("should fail");
        assert!(err.message.contains("'.foo'"), "got: {}", err.message);
    }

    #[test]
    fn vm_matches_eval_on_heap_corpus() {
        // Cross-check: every program here should produce identical
        // output on the bytecode VM and the tree-walker. The corpus
        // is the new session-10 surface area.
        let cases = [
            // Tuple ops that Snake exercises.
            "let pos = (3, 4)\nlet dir = (1, 0)\nlet next = pos + dir\nprint(next)\nprint(next * 10)\n",
            // List build + index + length.
            "let xs = [1, 2, 3]\nprint(xs.length)\nprint(xs[0])\nprint(xs[-1])\n",
            // List mutation.
            "let xs = [1]\nxs.append(2)\nxs.append(3)\nlet last = xs.pop_back()\nprint(xs)\nprint(last)\n",
            // For over range, list, tuple.
            "let sum = 0\nfor i in 1..5:\n    sum = sum + i\nprint(sum)\n",
            "for x in [10, 20, 30]:\n    print(x)\n",
            "for x in (\"a\", \"b\", \"c\"):\n    print(x)\n",
            // Nested for + break.
            "for i in 0..<3:\n    for j in 0..<3:\n        if j == i:\n            break\n        print((i, j))\n",
            // Interpolation with tuple in scope.
            "let p = (1, 2)\nprint(\"point = {p}, len = {p.x + p.y}\")\n",
            // `in` over various haystacks.
            "print(2 in [1, 2, 3])\nprint(5 in 0..<5)\nprint(\"hi\" in \"high\")\n",
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

    // --- Session 11: classes + methods + module builtins ---

    #[test]
    fn vm_runs_methods_test_program() {
        // Mirrors `tests/programs/methods.twe` exactly.
        let src = "item Counter:\n    value: 0\n\n    bump(amount):\n        self.value = self.value + amount\n\nlet c = Counter()\nprint(c.value)\nc.bump(5)\nprint(c.value)\nc.bump(7)\nprint(c.value)\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "0\n5\n12\n");
    }

    #[test]
    fn vm_instance_field_get_and_set() {
        let src = "entity Hero:\n    var hp = 100\n\nlet h = Hero()\nprint(h.hp)\nh.hp = 50\nprint(h.hp)\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "100\n50\n");
    }

    #[test]
    fn vm_method_with_args_and_return() {
        let src = "item Rect:\n    w: 0\n    h: 0\n\n    area():\n        return self.w * self.h\n\nlet r = Rect()\nr.w = 3\nr.h = 4\nprint(r.area())\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "12\n");
    }

    #[test]
    fn vm_compound_field_assignment() {
        let src = "item Score:\n    n: 0\n\nlet s = Score()\ns.n += 10\ns.n += 5\nprint(s.n)\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "15\n");
    }

    #[test]
    fn vm_self_outside_method_errors_at_compile() {
        let err = compile_err("print(self)\n");
        assert!(err.contains("`self`"), "got: {err}");
    }

    #[test]
    fn vm_field_default_must_be_const() {
        let err = compile_err("let g = 5\nitem Foo:\n    n: g\n");
        assert!(err.contains("literal constant"), "got: {err}");
    }

    #[test]
    fn vm_unknown_field_on_instance_errors() {
        let src = "item Foo:\n    a: 1\n\nlet f = Foo()\nprint(f.b)\n";
        let err = run_program(src).expect_err("should fail");
        assert!(err.message.contains("not defined"), "got: {}", err.message);
    }

    #[test]
    fn vm_unknown_method_on_instance_errors() {
        let src = "item Foo:\n    a: 1\n\nlet f = Foo()\nf.bar()\n";
        let err = run_program(src).expect_err("should fail");
        assert!(err.message.contains("bar"), "got: {}", err.message);
    }

    #[test]
    fn vm_constructor_with_args_errors() {
        let src = "item Foo:\n    a: 1\n\nlet f = Foo(5)\n";
        let err = run_program(src).expect_err("should fail");
        assert!(err.message.contains("constructor"), "got: {}", err.message);
    }

    #[test]
    fn vm_math_module_builtins() {
        // `math.min`, `.max`, `.abs` go through OP_INVOKE on Object.
        let out = run_program("print(math.min(3, 1))\nprint(math.max(3, 1))\nprint(math.abs(-7))\n")
            .expect("ok");
        assert_eq!(out, "1\n3\n7\n");
    }

    #[test]
    fn vm_math_sqrt() {
        let out = run_program("print(math.sqrt(9.0))\n").expect("ok");
        assert_eq!(out, "3.0\n");
    }

    #[test]
    fn vm_math_floor_ceil() {
        let out = run_program("print(math.floor(2.9))\nprint(math.ceil(2.1))\n").expect("ok");
        assert_eq!(out, "2\n3\n");
    }

    #[test]
    fn vm_module_field_get() {
        // `key.right` is a Bool field on the input Object — read-only
        // for tests, but exercises the Object field path.
        let out = run_program("print(key.right)\n").expect("ok");
        assert_eq!(out, "false\n");
    }

    #[test]
    fn vm_method_can_call_other_method_via_self() {
        let src = "item Math:\n    base: 10\n\n    double():\n        return self.base * 2\n\n    quad():\n        return self.double() * 2\n\nlet m = Math()\nprint(m.quad())\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "40\n");
    }

    #[test]
    fn vm_instances_have_independent_field_storage() {
        // Default fields are cloned per instance — mutating one
        // shouldn't mutate the other.
        let src = "item Box:\n    n: 0\n\nlet a = Box()\nlet b = Box()\na.n = 7\nprint(a.n)\nprint(b.n)\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "7\n0\n");
    }

    #[test]
    fn vm_matches_eval_on_class_corpus() {
        // Cross-check: every program here should produce identical
        // output on the bytecode VM and the tree-walker. This is
        // the session-11 canary against semantic drift on classes,
        // methods, self, and module builtins.
        let cases = [
            // The methods.twe contract.
            "item Counter:\n    value: 0\n\n    bump(amount):\n        self.value = self.value + amount\n\nlet c = Counter()\nprint(c.value)\nc.bump(5)\nprint(c.value)\nc.bump(7)\nprint(c.value)\n",
            // Compound field assignment.
            "item S:\n    n: 0\n\nlet s = S()\ns.n += 10\ns.n -= 3\nprint(s.n)\n",
            // Method that returns based on self fields.
            "item Rect:\n    w: 0\n    h: 0\n\n    area():\n        return self.w * self.h\n\nlet r = Rect()\nr.w = 6\nr.h = 7\nprint(r.area())\n",
            // Method calling sibling method via self.
            "item M:\n    base: 5\n\n    a():\n        return self.base + 1\n\n    b():\n        return self.a() * 10\n\nlet m = M()\nprint(m.b())\n",
            // Math module builtins.
            "print(math.abs(-7))\nprint(math.min(3, 1))\nprint(math.max(3.5, 2.5))\n",
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

    /// Tiny helper for tests that expect a compile error on a program.
    fn compile_err(src: &str) -> String {
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        crate::compiler::compile_program(&program)
            .err()
            .map(|e| e.message)
            .unwrap_or_default()
    }

    // --- Session 12: scenes + states + play loop ---

    /// Helper for tick-driven scene tests. Runs the top-level
    /// statements via `run`, then ticks `frames` times with `dt`.
    fn run_program_frames(src: &str, frames: u32, dt: f64) -> Result<String, RuntimeError> {
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let chunk = crate::compiler::compile_program(&program).expect("compile");
        let mut vm = VM::new();
        vm.run(&chunk)?;
        for _ in 0..frames {
            vm.tick(dt)?;
        }
        Ok(std::mem::take(&mut vm.out))
    }

    #[test]
    fn vm_scene_counter_runs_state_machine() {
        // Mirrors `tests/programs/scene_counter.twe`: scene with two
        // states, every-clock that increments, transition when done.
        let src = "scene Counter:\n    var ticks: int = 0\n\n    initial: counting\n\n    state counting:\n        every 100ms:\n            ticks += 1\n            print(ticks)\n            if ticks >= 3:\n                -> done\n\n    state done:\n";
        let out = run_program_frames(src, 5, 0.100).expect("ok");
        assert_eq!(out, "1\n2\n3\n");
    }

    #[test]
    fn vm_state_on_update_fires_each_frame() {
        // Mirrors `tests/programs/state_on_update.twe` shape.
        let src = "scene S:\n    var n: int = 0\n\n    initial: a\n\n    state a:\n        on update(dt):\n            n += 1\n            print(n)\n";
        let out = run_program_frames(src, 3, 0.016).expect("ok");
        assert_eq!(out, "1\n2\n3\n");
    }

    #[test]
    fn vm_top_level_on_update_fires_each_frame() {
        let src = "var n = 0\non update(dt):\n    n += 1\n    print(n)\n";
        let out = run_program_frames(src, 3, 0.016).expect("ok");
        assert_eq!(out, "1\n2\n3\n");
    }

    #[test]
    fn vm_scene_initial_on_entry_runs_at_boot() {
        // on_entry of the initial state runs at scene-instantiation
        // time (no tick needed).
        let src = "scene S:\n    initial: hello\n\n    state hello:\n        print(\"hi\")\n";
        let out = run_program_frames(src, 0, 0.016).expect("ok");
        assert_eq!(out, "hi\n");
    }

    #[test]
    fn vm_transition_re_enters_new_state_immediately() {
        // -> done from inside `start` should run done's on_entry
        // before tick returns.
        let src = "scene S:\n    initial: start\n\n    state start:\n        print(\"start\")\n        -> done\n\n    state done:\n        print(\"done\")\n";
        let out = run_program_frames(src, 0, 0.016).expect("ok");
        assert_eq!(out, "start\ndone\n");
    }

    #[test]
    fn vm_every_clock_catchup_is_capped() {
        // dt of 1.0s with interval 100ms would naturally fire 10
        // times; the cap drops it to MAX_CATCHUP_FIRES_PER_FRAME (8)
        // and resets the residual so next frame starts fresh.
        let src = "scene S:\n    var n: int = 0\n\n    initial: a\n\n    state a:\n        every 100ms:\n            n += 1\n            print(n)\n";
        let out = run_program_frames(src, 1, 1.0).expect("ok");
        assert_eq!(out, "1\n2\n3\n4\n5\n6\n7\n8\n");
    }

    #[test]
    fn vm_spawn_entity_pushes_to_active_entities() {
        // Spawn two Counter entities. tick once. Each entity's
        // update method fires.
        let src = "entity Counter:\n    var n = 0\n\n    update(dt):\n        n += 1\n        print(n)\n\nspawn Counter at (1, 0)\nspawn Counter at (2, 0)\n";
        let out = run_program_frames(src, 1, 0.016).expect("ok");
        assert_eq!(out, "1\n1\n");
    }

    #[test]
    fn vm_despawn_self_removes_at_end_of_frame() {
        // Mirrors `tests/programs/spawn_entities.twe` exactly:
        // entity prints n on each update, despawns at n>=2.
        let src = "entity Counter:\n    var n = 0\n\n    update(dt):\n        n += 1\n        print(n)\n        if n >= 2:\n            despawn self\n\nspawn Counter at (1, 0)\nspawn Counter at (2, 0)\n";
        let out = run_program_frames(src, 3, 0.016).expect("ok");
        // Frame 1: both fire (1, 1). Frame 2: both fire (2, 2) and
        // despawn. Frame 3: nothing.
        assert_eq!(out, "1\n1\n2\n2\n");
    }

    #[test]
    fn vm_entities_count_and_of_intrinsics() {
        // Mirrors `tests/programs/entity_query.twe` essentials.
        let src = "entity Mob:\n    var hp = 1\n    update(dt):\n        # nothing\n\nentity Bullet:\n    update(dt):\n        # nothing\n\nspawn Mob at (0, 0)\nspawn Mob at (1, 0)\nspawn Mob at (2, 0)\nspawn Bullet at (0, 0)\n\nprint(entities.count(Mob))\nprint(entities.count(Bullet))\n\nlet mobs = entities.of(Mob)\nprint(mobs.length)\nfor m in mobs:\n    print(m.hp)\n";
        let out = run_program_frames(src, 0, 0.016).expect("ok");
        assert_eq!(out, "3\n1\n3\n1\n1\n1\n");
    }

    #[test]
    fn vm_time_dt_is_live_each_frame() {
        // `time.dt` should equal whatever was passed to tick.
        let src = "scene S:\n    var sum = 0.0\n\n    initial: a\n\n    state a:\n        every 50ms:\n            sum += time.dt\n            print(sum)\n";
        let out = run_program_frames(src, 1, 0.050).expect("ok");
        assert_eq!(out, "0.05\n");
    }

    #[test]
    fn vm_bare_name_writes_to_self_field_in_state() {
        // `ticks += 1` inside a state body should mutate self.ticks
        // — that's the bare-name → self-field rewrite.
        let src = "scene S:\n    var ticks: int = 0\n\n    initial: counting\n\n    state counting:\n        every 100ms:\n            ticks += 1\n            print(ticks)\n";
        let out = run_program_frames(src, 3, 0.100).expect("ok");
        assert_eq!(out, "1\n2\n3\n");
    }

    #[test]
    fn vm_field_default_must_be_const_for_scene_too() {
        let err = compile_err(
            "let g = 5\nscene S:\n    var n: int = g\n    initial: a\n    state a:\n",
        );
        assert!(err.contains("literal constant"), "got: {err}");
    }

    #[test]
    fn vm_unknown_initial_state_errors_at_compile() {
        let err = compile_err("scene S:\n    initial: missing\n    state a:\n");
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn vm_on_render_errors_pointing_at_session_13() {
        let err = compile_err(
            "scene S:\n    initial: a\n    state a:\n        on render():\n            print(1)\n",
        );
        assert!(err.contains("session 13"), "got: {err}");
    }

    #[test]
    fn vm_matches_eval_on_play_loop_corpus() {
        // Cross-check the play-loop programs through both interpreters
        // using their respective tick drivers. Mirrors how the eval
        // tests in tests/eval.rs run the same files via run_with_frames.
        let cases: &[(&str, u32, f64)] = &[
            // scene_counter.twe: 5 frames @ 100ms produces 1,2,3 then idle.
            ("scene Counter:\n    var ticks: int = 0\n\n    initial: counting\n\n    state counting:\n        every 100ms:\n            ticks += 1\n            print(ticks)\n            if ticks >= 3:\n                -> done\n\n    state done:\n", 5, 0.100),
            // state_on_update: 3 frames produces 1,2,3.
            ("scene S:\n    var n: int = 0\n    initial: a\n    state a:\n        on update(dt):\n            n += 1\n            print(n)\n", 3, 0.016),
            // top-level on_update: 3 frames produces 1,2,3.
            ("var n = 0\non update(dt):\n    n += 1\n    print(n)\n", 3, 0.016),
            // spawn + entity update with despawn.
            ("entity Counter:\n    var n = 0\n    update(dt):\n        n += 1\n        print(n)\n        if n >= 2:\n            despawn self\n\nspawn Counter at (1, 0)\nspawn Counter at (2, 0)\n", 3, 0.016),
            // entities.count after spawning.
            ("entity Mob:\n    var hp = 1\n    update(dt):\n        # nothing\n\nspawn Mob at (0, 0)\nspawn Mob at (1, 0)\nprint(entities.count(Mob))\n", 0, 0.016),
        ];
        for (src, frames, dt) in cases {
            let bytecode_out = run_program_frames(src, *frames, *dt)
                .unwrap_or_else(|e| panic!("bytecode failed on `{src}`: {e}"));
            let walker_out = crate::eval::run_with_frames(
                &parser::parse(&lexer::lex(src).expect("lex")).expect("parse"),
                *frames,
                *dt,
            )
            .unwrap_or_else(|e| panic!("walker failed on `{src}`: {e}"));
            assert_eq!(
                bytecode_out, walker_out,
                "results diverge on `{src}`",
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
