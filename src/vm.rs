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
use crate::tagged_value::TaggedValue;
use crate::value::{Env, RuntimeError, Value};

/// Cap on how many times a single `every <duration>:` clock can
/// fire in one frame. Same value as `eval::MAX_CATCHUP_FIRES_PER_FRAME`
/// — eight 16ms ticks ≈ 128ms of catch-up, comfortably absorbing a
/// slow first frame while bounded so a long stall can't lock the
/// runtime in catch-up forever. Closes Phase-2 frustration F4 for
/// the bytecode VM too.
const MAX_CATCHUP_FIRES_PER_FRAME: u32 = 8;

/// Convert a runtime value to seconds for `wait <duration>`. Mirrors
/// `eval::quantity_to_seconds` — kept separate so the bytecode VM
/// doesn't reach into the tree-walker's eval module. The two will
/// reconcile when the value layer unifies (post-NaN-tagging).
fn wait_duration_to_seconds(v: &Value, line: u32) -> Result<f64, RuntimeError> {
    if v.is_quantity() {
        let (value, unit) = v.as_quantity();
        match unit.as_str() {
            "s" => Ok(value),
            "ms" => Ok(value / 1000.0),
            "min" => Ok(value * 60.0),
            "h" => Ok(value * 3600.0),
            other => Err(RuntimeError {
                line,
                col: 0,
                message: format!(
                    "wait <duration> needs a time unit (s, ms, min, h), got '{other}'"
                ),
                help: None,
            }),
        }
    } else if v.is_float() {
        let f = v.as_float();
        Ok(f)
    } else if v.is_int_or_boxed_int() {
        let n = v.as_int();
        Ok(n as f64)
    } else {
        let other = *v;
        Err(RuntimeError {
            line,
            col: 0,
            message: format!(
                "wait <duration> expects a duration quantity, got {}",
                other.type_name()
            ),
            help: Some("e.g. `wait 0.5s` or `wait 250ms`".to_string()),
        })
    }
}

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
    /// v0.2 Phase 8.5 session 8c: value stack on `TaggedValue`.
    /// Pattern-matching opcodes still operate on legacy `Value`
    /// — the helpers `push` / `pop` / `pop_n` / `peek_*`
    /// shim at the boundary. Inner heap pattern matches migrate
    /// to `TaggedValue` predicates in 8f when `Value` deletes.
    stack: Vec<TaggedValue>,
    frames: Vec<CallFrame>,
    /// v0.2 Phase 8.5 session 8c: globals also on `TaggedValue`.
    /// Mirrors stack — get/set converts via `to_legacy` /
    /// `from_legacy` at the boundary.
    globals: HashMap<String, TaggedValue>,
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
    /// Phase 11 session 10: bytecode-VM mirror of eval's
    /// `death_handlers`. Keyed by class name (matched against
    /// `BcInstance.class.name` at fire time); each entry is a list
    /// of handler functions registered in source order, each
    /// invoked once per dying instance.
    death_handlers: HashMap<String, Vec<crate::bytecode::BcDeathHandler>>,
    /// v0.2 session 7: when dispatch is currently inside (or
    /// descended from) a state.on_entry call, this is the index
    /// in `self.frames` of the state-entry frame. None
    /// otherwise. `OP_WAIT` uses it to know how much of the call
    /// stack to capture for the multi-frame fiber save; the
    /// runtime gate ("wait outside state on_entry is an error")
    /// also keys off this.
    state_entry_frame_depth: Option<usize>,
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
            globals.insert(name, value);
        }
        // Replace the stdlib `entities` Object (which routes through
        // tree-walker builtins) with a VM-tagged one so OP_INVOKE
        // can dispatch to our own active_entities.
        globals.insert(
            "entities".to_string(),
            Value::from_object(Rc::new(RefCell::new(crate::value::Object {
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
            death_handlers: HashMap::new(),
            state_entry_frame_depth: None,
            out: String::new(),
        }
    }

    /// v0.2 Phase 8.5 session 8h: walk every GC root reachable from
    /// the bytecode VM and mark them. Called from a safepoint inside
    /// `gc_collect_with(|| vm.scan_roots())`.
    ///
    /// Roots: the value stack (every live local + temporary), every
    /// global, the held builtin Env (its bindings + active state),
    /// the active scene's instance fields + fiber stack, every
    /// active entity's instance fields + fiber stack, and every
    /// active CallFrame's compiled function constants pool.
    ///
    /// The constants pool of a running function is held as a naked
    /// `Rc<BcFunction>` in `CallFrame::function` — it isn't otherwise
    /// reachable through any TaggedValue, so the safepoint must mark
    /// every constant explicitly or string / function / class
    /// constants get swept while a function is running.
    pub fn scan_roots(&self) {
        for v in &self.stack {
            crate::heap::mark_value(v);
        }
        for v in self.globals.values() {
            crate::heap::mark_value(v);
        }
        // The held builtin Env duplicates some stdlib state from globals
        // but may also hold bindings the VM never mirrored back. Scan it
        // so nothing reachable through builtins gets swept.
        self.builtin_env.scan_roots();
        if let Some(scene) = &self.active_scene {
            crate::bytecode::mark_bc_instance(&scene.borrow());
        }
        for ent in &self.active_entities {
            crate::bytecode::mark_bc_instance(&ent.borrow());
        }
        for frame in &self.frames {
            crate::heap::mark_bc_function_constants(&frame.function);
        }
        if let Some(f) = &self.on_update {
            crate::heap::mark_bc_function_constants(f);
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
        self.push(Value::from_bc_function(script.clone()));
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
        // Hoist the active frame's function and ip into locals so the
        // dispatch loop skips the per-instruction `self.frames.last()`
        // / `last_mut()` Vec lookups. Sync back to the frame slot
        // only on Call / Invoke (which may push a frame and continue
        // dispatching it inline) and Return (which always pops).
        // Per *Crafting Interpreters* §24.7: the single biggest
        // dispatch win in the Lox VM.
        //
        // Frame-mutating helpers that call `invoke_method_value`
        // internally (enter_state, tick_scene, seed_particle_emitter,
        // ...) push and pop their own frames in a balanced way via a
        // nested `dispatch()` invocation. From this outer dispatch's
        // perspective, the frame stack is unchanged after they return,
        // so the cached locals stay valid — no sync/reload needed
        // around InitScene / Spawn / Despawn / Transition.
        let mut current_func = Rc::clone(&self.frames.last().unwrap().function);
        let mut ip = self.frames.last().unwrap().ip;
        // v0.2 Phase 8.5 session 8i: also cache slot_base so OP_GET_LOCAL
        // / OP_SET_LOCAL dispatch doesn't pay a Vec::last + unwrap on
        // every iteration. Reload it any time `reload!` runs.
        let mut slot_base = self.frames.last().unwrap().slot_base;

        macro_rules! sync_ip {
            () => {
                self.frames.last_mut().unwrap().ip = ip;
            };
        }
        macro_rules! reload {
            () => {
                let top = self.frames.last().unwrap();
                current_func = Rc::clone(&top.function);
                ip = top.ip;
                slot_base = top.slot_base;
            };
        }
        macro_rules! read_byte {
            () => {{
                let b = current_func.chunk.code[ip];
                ip += 1;
                b
            }};
        }
        macro_rules! read_u16 {
            () => {{
                let hi = current_func.chunk.code[ip] as u16;
                let lo = current_func.chunk.code[ip + 1] as u16;
                ip += 2;
                (hi << 8) | lo
            }};
        }
        macro_rules! read_string_const {
            ($idx:expr, $line:expr) => {{
                {
let __t = &current_func.chunk.constants[$idx];
if __t.is_str() {
let s = __t.as_string();
Ok(s.clone())
} else {
let other = __t.clone();
Err(RuntimeError {
                        line: $line,
                        col: 0,
                        message: format!(
                            "vm: expected a string constant for global name, got {}",
                            other.type_name()
                        ),
                        help: Some(
                            "compiler bug — global ops must point at a Value::Str".to_string(),
                        ),
                    })
}
}
            }};
        }

        loop {
            if ip >= current_func.chunk.code.len() {
                // Defensive — the compiler always emits OP_RETURN, so
                // this is a fallthrough only on a malformed chunk.
                sync_ip!();
                return Ok(self.stack.pop().unwrap_or(Value::NIL));
            }
            // v0.2 Phase 8.5 session 8h: GC safepoint between bytecode
            // instructions. Threshold-gated so non-allocating dispatch
            // pays only the cheap `bytes_allocated >= threshold` check;
            // a real collect fires only once per ~2× live-set growth.
            if crate::heap::gc_should_collect() {
                sync_ip!();
                crate::heap::gc_collect_with(|| self.scan_roots());
            }
            let line = current_func.chunk.lines[ip];
            let op = OpCode::from_u8(current_func.chunk.code[ip]);
            ip += 1;

            match op {
                OpCode::Constant => {
                    let idx = read_byte!() as usize;
                    let value = current_func.chunk.constants[idx];
                    self.push(value);
                }
                OpCode::Nil => self.push(Value::NIL),
                OpCode::True => self.push(Value::TRUE),
                OpCode::False => self.push(Value::FALSE),
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
                    self.push(Value::from_bool(!is_truthy(&v)));
                }
                OpCode::Equal => {
                    let r = self.pop()?;
                    let l = self.pop()?;
                    self.push(Value::from_bool(values_equal(&l, &r)));
                }
                OpCode::NotEqual => {
                    let r = self.pop()?;
                    let l = self.pop()?;
                    self.push(Value::from_bool(!values_equal(&l, &r)));
                }
                OpCode::Less => self.compare("<", line, |a, b| a < b, |a, b| a < b)?,
                OpCode::LessEqual => self.compare("<=", line, |a, b| a <= b, |a, b| a <= b)?,
                OpCode::Greater => self.compare(">", line, |a, b| a > b, |a, b| a > b)?,
                OpCode::GreaterEqual => self.compare(">=", line, |a, b| a >= b, |a, b| a >= b)?,
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
                        // can pop it.
                        self.push(result);
                        return Ok(result);
                    }
                    if self.frames.is_empty() {
                        // Script frame ended at top level.
                        return Ok(result);
                    }
                    self.push(result);
                    reload!();
                }
                OpCode::Wait => {
                    // Phase 5 fibers + v0.2 sessions 2c + 7. Pop
                    // the duration value, save the *entire* frame
                    // chain from the state-entry frame down to
                    // the current top, plus the value-stack slice
                    // covering all those frames' locals. Then
                    // collapse all those frames back to before the
                    // state-entry call, returning Nil.
                    sync_ip!();
                    let dur = self.pop()?;
                    let secs = wait_duration_to_seconds(&dur, line)?;
                    let scene = self.active_scene.clone().ok_or_else(|| RuntimeError {
                        line,
                        col: 0,
                        message: "`wait` requires an active scene".to_string(),
                        help: Some(
                            "`wait` only fires from inside a state's on_entry body, which executes under an active scene".to_string(),
                        ),
                    })?;
                    let entry_depth = self
                        .state_entry_frame_depth
                        .ok_or_else(|| RuntimeError {
                            line,
                            col: 0,
                            message:
                                "`wait` is only valid in a state's on_entry body or a function called from there"
                                    .to_string(),
                            help: Some(
                                "wait fires from a fiber rooted in `state <name>:`'s on_entry; calls from on_update / every / top-level can't suspend"
                                    .to_string(),
                            ),
                        })?;

                    // sync_ip wrote the current frame's `ip` back
                    // into self.frames[len-1]. That ip is one byte
                    // past OP_WAIT — exactly where resume should
                    // continue. The remaining frames already carry
                    // their own ip values from the OP_CALL that
                    // pushed them.
                    let bottom_slot_base = self.frames[entry_depth].slot_base;
                    let mut saved_frames: Vec<crate::bytecode::BcFiberFrame> =
                        Vec::with_capacity(self.frames.len() - entry_depth);
                    for f in self.frames.iter().skip(entry_depth) {
                        saved_frames.push(crate::bytecode::BcFiberFrame {
                            function: Rc::clone(&f.function),
                            ip: f.ip,
                            slot_base_offset: f.slot_base - bottom_slot_base,
                        });
                    }
                    let saved_stack: Vec<Value> = self.stack[bottom_slot_base..].to_vec();

                    {
                        let mut inst = scene.borrow_mut();
                        inst.fiber_frames = saved_frames;
                        inst.fiber_stack = saved_stack;
                        inst.entry_wait_remaining = secs;
                    }

                    // Collapse: drop the frames + stack region the
                    // fiber owned, then leave a synthetic Nil for
                    // the caller of the state-entry's invoke (which
                    // expects a return value).
                    self.frames.truncate(entry_depth);
                    self.stack.truncate(bottom_slot_base);
                    self.state_entry_frame_depth = None;
                    if self.frames.len() < target_depth {
                        self.push(Value::NIL);
                        return Ok(Value::NIL);
                    }
                    if self.frames.is_empty() {
                        return Ok(Value::NIL);
                    }
                    self.push(Value::NIL);
                    reload!();
                }
                OpCode::GetLocal => {
                    let slot = read_byte!() as usize;
                    let abs = slot_base + slot;
                    let v = self.slot_get(abs).ok_or_else(|| RuntimeError {
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
                    let slot = read_byte!() as usize;
                    let abs = slot_base + slot;
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
                    // SetLocal: peek top, copy into slot. Both sides are
                    // TaggedValue — clone the slot directly without
                    // round-tripping through legacy.
                    let top = self.stack.last().cloned().unwrap();
                    self.stack[abs] = top;
                }
                OpCode::JumpIfFalse => {
                    let offset = read_u16!();
                    let v = self.pop()?;
                    if !is_truthy(&v) {
                        ip += offset as usize;
                    }
                }
                OpCode::JumpIfFalsePeek => {
                    let offset = read_u16!();
                    let truthy =
                        self.peek_top()
                            .as_ref()
                            .map(is_truthy)
                            .ok_or_else(|| RuntimeError {
                                line,
                                col: 0,
                                message: "vm: stack underflow on JumpIfFalsePeek".to_string(),
                                help: None,
                            })?;
                    if !truthy {
                        ip += offset as usize;
                    }
                }
                OpCode::JumpIfTruePeek => {
                    let offset = read_u16!();
                    let truthy =
                        self.peek_top()
                            .as_ref()
                            .map(is_truthy)
                            .ok_or_else(|| RuntimeError {
                                line,
                                col: 0,
                                message: "vm: stack underflow on JumpIfTruePeek".to_string(),
                                help: None,
                            })?;
                    if truthy {
                        ip += offset as usize;
                    }
                }
                OpCode::Jump => {
                    let offset = read_u16!();
                    ip += offset as usize;
                }
                OpCode::Loop => {
                    let offset = read_u16!();
                    ip = ip
                        .checked_sub(offset as usize)
                        .ok_or_else(|| RuntimeError {
                            line,
                            col: 0,
                            message: "vm: OP_LOOP offset underflow".to_string(),
                            help: None,
                        })?;
                }
                OpCode::DefineGlobal => {
                    let idx = read_byte!() as usize;
                    let name = read_string_const!(idx, line)?;
                    // Pop directly as TaggedValue to avoid the
                    // legacy round-trip; globals are TaggedValue.
                    let tagged = self.stack.pop().ok_or_else(|| RuntimeError {
                        line,
                        col: 0,
                        message: "vm: stack underflow on DefineGlobal".to_string(),
                        help: None,
                    })?;
                    self.globals.insert(name, tagged);
                }
                OpCode::GetGlobal => {
                    let idx = read_byte!() as usize;
                    let name = read_string_const!(idx, line)?;
                    let tagged =
                        self.globals
                            .get(name.as_str())
                            .cloned()
                            .ok_or_else(|| RuntimeError {
                                line,
                                col: 0,
                                message: format!("name `{name}` is not defined"),
                                help: Some(format!(
                                    "declare it with `let {name} = ...` before using it"
                                )),
                            })?;
                    self.stack.push(tagged);
                }
                OpCode::SetGlobal => {
                    let idx = read_byte!() as usize;
                    let name = read_string_const!(idx, line)?;
                    if !self.globals.contains_key(name.as_str()) {
                        return Err(RuntimeError {
                            line,
                            col: 0,
                            message: format!("name `{name}` is not defined"),
                            help: Some(format!(
                                "declare it with `let {name} = ...` before assigning"
                            )),
                        });
                    }
                    let tagged = self.stack.last().cloned().ok_or_else(|| RuntimeError {
                        line,
                        col: 0,
                        message: "vm: stack underflow on SetGlobal".to_string(),
                        help: None,
                    })?;
                    self.globals.insert(name, tagged);
                }
                OpCode::Call => {
                    let arg_count = read_byte!() as usize;
                    let frames_before = self.frames.len();
                    sync_ip!();
                    self.call_value(arg_count, line)?;
                    if self.frames.len() != frames_before {
                        // call_value pushed a frame for a BcFunction;
                        // dispatch resumes inside it. Builtin / BcClass
                        // calls don't push a frame so we stay put.
                        reload!();
                    }
                }
                OpCode::BuildTuple => {
                    let n = read_byte!() as usize;
                    let elems = self.pop_n(n, line)?;
                    self.push(Value::from_tuple(Rc::new(elems)));
                }
                OpCode::BuildList => {
                    let n = read_byte!() as usize;
                    let elems = self.pop_n(n, line)?;
                    self.push(Value::from_list(Rc::new(RefCell::new(elems))));
                }
                OpCode::BuildRange => {
                    let exclusive = read_byte!() != 0;
                    let end = self.pop()?;
                    let start = self.pop()?;
                    if start.is_int_or_boxed_int() && end.is_int_or_boxed_int() {
                        self.push(Value::from_range(start.as_int(), end.as_int(), exclusive));
                    } else {
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
                OpCode::Index => {
                    let idx = self.pop()?;
                    let obj = self.pop()?;
                    let v = index_get(&obj, &idx, line)?;
                    self.push(v);
                }
                OpCode::ToStr => {
                    let v = self.pop()?;
                    self.push(Value::from_string(v.display()));
                }
                OpCode::Interp => {
                    let n = read_byte!() as usize;
                    let parts = self.pop_n(n, line)?;
                    let mut out = String::new();
                    for p in &parts {
                        if p.is_str() {
                            let s = p.as_string();
                            out.push_str(s.as_str())
                        } else {
                            let other = *p;
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
                    self.push(Value::from_string(out));
                }
                OpCode::In => {
                    let haystack = self.pop()?;
                    let needle = self.pop()?;
                    let found = value_in(&needle, &haystack, line)?;
                    self.push(Value::from_bool(found));
                }
                OpCode::GetField => {
                    let idx = read_byte!() as usize;
                    let name = read_string_const!(idx, line)?;
                    let recv = self.pop()?;
                    let v = field_get(&recv, name.as_str(), line)?;
                    self.push(v);
                }
                OpCode::SetField => {
                    let idx = read_byte!() as usize;
                    let name = read_string_const!(idx, line)?;
                    let value = self.pop()?;
                    let recv = self.pop()?;
                    field_set(&recv, name.as_str(), value, line)?;
                    self.push(value);
                }
                OpCode::InitScene => {
                    let class_val = self.pop()?;
                    let class = if class_val.is_bc_class() {
                        class_val.as_bc_class()
                    } else {
                        let other = class_val;
                        return Err(RuntimeError {
                            line,
                            col: 0,
                            message: format!(
                                "OP_INIT_SCENE expected a class, got {}",
                                other.type_name()
                            ),
                            help: Some("compiler bug".to_string()),
                        });
                    };
                    let inst = {
                        let __t = instantiate_bc(class.clone());
                        if __t.is_bc_instance() {
                            __t.as_bc_instance()
                        } else {
                            unreachable!()
                        }
                    };
                    self.active_scene = Some(inst.clone());
                    if let Some(start) = class.initial_state.clone() {
                        // enter_state runs nested invocations via
                        // invoke_method_value (separate dispatch);
                        // frames.len() unchanged after return.
                        sync_ip!();
                        self.enter_state(&inst, &start)?;
                    }
                }
                OpCode::Spawn => {
                    let with_at = read_byte!() != 0;
                    let class_val = self.pop()?;
                    let at_value = if with_at { Some(self.pop()?) } else { None };
                    let class = if class_val.is_bc_class() {
                        class_val.as_bc_class()
                    } else {
                        let other = class_val;
                        return Err(RuntimeError {
                            line,
                            col: 0,
                            message: format!("`spawn` expected a class, got {}", other.type_name()),
                            help: None,
                        });
                    };
                    let inst = {
                        let __t = instantiate_bc(class.clone());
                        if __t.is_bc_instance() {
                            __t.as_bc_instance()
                        } else {
                            unreachable!()
                        }
                    };
                    if let Some(at) = at_value {
                        inst.borrow_mut().insert_field("pos".to_string(), at);
                    }
                    if class.kind == "particles" {
                        sync_ip!();
                        self.seed_particle_emitter(&inst, at_value, line)?;
                    }
                    self.active_entities.push(inst.clone());
                    if let Some(start) = class.initial_state.clone() {
                        sync_ip!();
                        self.enter_state(&inst, &start)?;
                    }
                }
                OpCode::Despawn => {
                    let target = self.pop()?;
                    if target.is_bc_instance() {
                        let rc = target.as_bc_instance();
                        rc.borrow_mut().despawned = true;
                    } else {
                        let other = target;
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
                OpCode::Transition => {
                    let idx = read_byte!() as usize;
                    let target = read_string_const!(idx, line)?;
                    self.transitioning = Some(target);
                }
                OpCode::SetOnUpdate => {
                    let idx = read_byte!() as usize;
                    let value = current_func.chunk.constants[idx];
                    if value.is_bc_function() {
                        let func = value.as_bc_function();
                        self.on_update = Some(func);
                    } else {
                        let other = value;
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
                OpCode::RegisterDeathHandler => {
                    let class_idx = read_byte!() as usize;
                    let func_idx = read_byte!() as usize;
                    let class_v = current_func.chunk.constants[class_idx];
                    let func_v = current_func.chunk.constants[func_idx];
                    if !class_v.is_str() {
                        return Err(RuntimeError {
                            line,
                            col: 0,
                            message: "OP_REGISTER_DEATH_HANDLER expected a string class name"
                                .to_string(),
                            help: Some("compiler bug".to_string()),
                        });
                    }
                    if !func_v.is_bc_function() {
                        return Err(RuntimeError {
                            line,
                            col: 0,
                            message: "OP_REGISTER_DEATH_HANDLER expected a function".to_string(),
                            help: Some("compiler bug".to_string()),
                        });
                    }
                    let class_name = class_v.as_string().clone();
                    let func = func_v.as_bc_function();
                    self.death_handlers.entry(class_name).or_default().push(
                        crate::bytecode::BcDeathHandler {
                            // The compiler doesn't ship the param name into
                            // the runtime — slot 1 in the function body is
                            // already bound to the argument we pass.
                            param: String::new(),
                            func,
                        },
                    );
                }
                OpCode::Invoke => {
                    let name_idx = read_byte!() as usize;
                    let arg_count = read_byte!() as usize;
                    let name = read_string_const!(name_idx, line)?;
                    let frames_before = self.frames.len();
                    sync_ip!();
                    self.invoke_method(name.as_str(), arg_count, line)?;
                    if self.frames.len() != frames_before {
                        // BcInstance method dispatch pushed a frame;
                        // List/Range/Object intrinsics didn't.
                        reload!();
                    }
                }
                OpCode::ForNext => {
                    let base_slot = read_byte!() as usize;
                    let exit_offset = read_u16!();
                    let abs_iter = self.frames.last().unwrap().slot_base + base_slot;
                    let abs_counter = abs_iter + 1;
                    let counter_val = self.slot_get(abs_counter);
                    let counter = match counter_val.as_ref() {
                        Some(t) if t.is_int_or_boxed_int() => t.as_int(),
                        other => {
                            return Err(RuntimeError {
                                line,
                                col: 0,
                                message: format!(
                                    "vm: for-loop counter not an int (got {})",
                                    other.map(|v| v.type_name()).unwrap_or("missing")
                                ),
                                help: Some("compiler bug".to_string()),
                            });
                        }
                    };
                    let iter_value = self.slot_get(abs_iter).unwrap_or(Value::NIL);
                    let next = if iter_value.is_range() {
                        let (start, end, exclusive) = iter_value.as_range();
                        let limit = if exclusive { end } else { end + 1 };
                        let cur = start + counter;
                        if cur < limit {
                            Some(Value::from_int(cur))
                        } else {
                            None
                        }
                    } else if iter_value.is_list() {
                        let rc = iter_value.as_list();
                        let v = rc.borrow();
                        if (counter as usize) < v.len() {
                            Some(v[counter as usize])
                        } else {
                            None
                        }
                    } else if iter_value.is_tuple() {
                        let elems = iter_value.as_tuple();
                        if (counter as usize) < elems.len() {
                            Some(elems[counter as usize])
                        } else {
                            None
                        }
                    } else {
                        let other = iter_value;
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
                    };
                    match next {
                        Some(elem) => {
                            self.slot_set(abs_counter, Value::from_int(counter + 1));
                            self.push(elem);
                        }
                        None => {
                            ip += exit_offset as usize;
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
        let callee_idx =
            self.stack
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
        let callee = self.stack[callee_idx];
        if callee.is_bc_function() {
            self.push_call_frame(callee.as_bc_function(), callee_idx, arg_count, line)
        } else if callee.is_bc_class() {
            let class = callee.as_bc_class();
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
        } else if callee.is_builtin() {
            let (name, params, func) = callee.as_builtin();
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
        } else {
            Err(RuntimeError {
                line,
                col: 0,
                message: format!(
                    "tried to call a {} (only functions and classes are callable)",
                    callee.type_name()
                ),
                help: None,
            })
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
            let dummy_recv = Value::from_bc_function(handler.clone());
            self.invoke_method_value(handler, dummy_recv, &[Value::from_float(dt)])?;
        }
        if let Some(scene) = self.active_scene.clone() {
            // dispatch_key_press fires BEFORE tick_scene (matches
            // eval::tick_frame ordering — input handlers see the
            // pre-tick state).
            self.dispatch_key_press(&scene)?;
            self.tick_scene(&scene, dt)?;
        }
        let entities = self.active_entities.clone();
        for entity in entities {
            if entity.borrow().despawned {
                continue;
            }
            let class = entity.borrow().class.clone();
            if class.kind == "particles" {
                self.tick_particle_emitter(&entity, dt)?;
                continue;
            }
            if let Some(method) = class.methods.get("update").cloned() {
                self.invoke_method_value(
                    method,
                    Value::from_bc_instance(entity.clone()),
                    &[Value::from_float(dt)],
                )?;
            }
        }
        // Phase 11 session 10: fire `on <Class>.death(e):` handlers
        // for any entity flagged this frame whose handler hasn't run
        // yet. Mirrors the eval-side `prune_despawned` ordering: the
        // dying entity is still in `active_entities` when its handler
        // runs, so the handler body can read its fields.
        if !self.death_handlers.is_empty() {
            let snapshot = self.active_entities.clone();
            for entity in snapshot {
                let (despawned, fired, class_name) = {
                    let i = entity.borrow();
                    (i.despawned, i.death_fired, i.class.name.clone())
                };
                if !despawned || fired {
                    continue;
                }
                entity.borrow_mut().death_fired = true;
                let handlers = self.death_handlers.get(&class_name).cloned();
                if let Some(handlers) = handlers {
                    let recv = Value::from_bc_instance(entity.clone());
                    for handler in handlers {
                        self.invoke_method_value(handler.func, recv, &[recv])?;
                    }
                }
            }
        }
        self.active_entities.retain(|e| !e.borrow().despawned);
        Ok(())
    }

    /// Drive one render frame. Fires the active scene's current
    /// state's `on render():` handler (if any), then each entity's
    /// `render()` method. `builtin_env.in_render` is toggled around
    /// the call so drawing builtins (`rect`, `text`, `circle`, ...)
    /// pass their `require_render` gate. Particles without a custom
    /// `render()` defer to the engine's per-particle drawing path
    /// (skipped in headless tests — matches the tree-walker's
    /// `env.in_render` check inside `render_particle_emitter`).
    pub fn render(&mut self) -> Result<(), RuntimeError> {
        self.builtin_env.in_render = true;
        let result = self.render_inner();
        self.builtin_env.in_render = false;
        result
    }

    fn render_inner(&mut self) -> Result<(), RuntimeError> {
        if let Some(scene) = self.active_scene.clone() {
            let body = {
                let inst = scene.borrow();
                inst.current_state
                    .as_ref()
                    .and_then(|n| inst.class.states.get(n))
                    .and_then(|s| s.on_render.clone())
            };
            if let Some(body) = body {
                self.invoke_method_value(body, Value::from_bc_instance(scene.clone()), &[])?;
            }
            // Apply state transitions raised during on_render — modal
            // states like a level-up picker put their button widgets
            // in render and rely on `-> playing` to dismiss themselves
            // (mirror of eval::render_frame).
            if let Some(target) = self.transitioning.take() {
                self.enter_state(&scene, &target)?;
            }
        }
        let entities = self.active_entities.clone();
        for entity in entities {
            if entity.borrow().despawned {
                continue;
            }
            let class = entity.borrow().class.clone();
            if let Some(method) = class.methods.get("render").cloned() {
                self.invoke_method_value(method, Value::from_bc_instance(entity.clone()), &[])?;
            }
        }
        Ok(())
    }

    /// Mirror of `eval::dispatch_key_press`. Reads the `key_press`
    /// Object's bool fields; for each true field that the active
    /// state has a handler for, invokes the handler. A transition
    /// inside a handler short-circuits the rest.
    fn dispatch_key_press(&mut self, scene: &Rc<RefCell<BcInstance>>) -> Result<(), RuntimeError> {
        let key_press_val = self.globals.get("key_press");
        let pressed: Vec<String> = if let Some(t) = key_press_val.as_ref() {
            if t.is_object() {
                let rc = t.as_object();
                let result: Vec<String> = rc
                    .borrow()
                    .fields
                    .iter()
                    .filter_map(|(k, v)| {
                        if v.is_bool() && v.as_bool() {
                            Some(k.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                result
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        };
        if pressed.is_empty() {
            return Ok(());
        }
        let bodies: Vec<Rc<BcFunction>> = {
            let inst = scene.borrow();
            let state = match inst
                .current_state
                .as_ref()
                .and_then(|n| inst.class.states.get(n))
            {
                Some(s) => s.clone(),
                None => return Ok(()),
            };
            pressed
                .iter()
                .filter_map(|k| state.on_key_press.get(k).cloned())
                .collect()
        };
        for body in bodies {
            self.invoke_method_value(body, Value::from_bc_instance(scene.clone()), &[])?;
            if let Some(target) = self.transitioning.take() {
                self.enter_state(scene, &target)?;
                break;
            }
        }
        Ok(())
    }

    /// Seed a particle emitter on `spawn`: read `count` and
    /// `lifetime` from the instance's fields, build that many
    /// Particle Objects (with default fields the runtime can age),
    /// fire the class's `on_spawn(p)` for each particle if present,
    /// then stash the list as the hidden `__particles` field.
    /// Mirrors `eval::seed_particle_emitter`.
    fn seed_particle_emitter(
        &mut self,
        emitter: &Rc<RefCell<BcInstance>>,
        at: Option<Value>,
        line: u32,
    ) -> Result<(), RuntimeError> {
        let (count, lifetime, on_spawn) = {
            let inst = emitter.borrow();
            let count = match inst.get_field("count") {
                Some(t) if t.is_int_or_boxed_int() && t.as_int() >= 0 => t.as_int() as usize,
                Some(other) => {
                    return Err(RuntimeError {
                        line,
                        col: 0,
                        message: format!(
                            "particles `count` must be a non-negative int, got {}",
                            other.type_name()
                        ),
                        help: None,
                    });
                }
                None => 16,
            };
            let lifetime = {
                let __opt = inst.get_field("lifetime");
                if let Some(__t) = (__opt).as_ref() {
                    if __t.is_float() {
                        __t.as_float()
                    } else if __t.is_int_or_boxed_int() {
                        let n = __t.as_int();
                        n as f64
                    } else if __t.is_quantity() {
                        let (value, _) = __t.as_quantity();
                        value
                    } else {
                        let other = *__t;
                        return Err(RuntimeError {
                            line,
                            col: 0,
                            message: format!(
                                "particles `lifetime` must be a number or duration, got {}",
                                other.type_name()
                            ),
                            help: Some("e.g. `lifetime = 0.6` (seconds)".to_string()),
                        });
                    }
                } else {
                    1.0
                }
            };
            let on_spawn = inst.class.methods.get("on_spawn").cloned();
            (count, lifetime, on_spawn)
        };
        let initial_pos = at.unwrap_or_else(|| {
            Value::from_tuple(Rc::new(vec![
                Value::from_float(0.0),
                Value::from_float(0.0),
            ]))
        });
        let mut particles: Vec<Value> = Vec::with_capacity(count);
        for _ in 0..count {
            let p = make_particle(&initial_pos, lifetime);
            if let Some(method) = on_spawn.clone() {
                self.invoke_method_value(
                    method,
                    Value::from_bc_instance(emitter.clone()),
                    std::slice::from_ref(&p),
                )?;
            }
            particles.push(p);
        }
        emitter.borrow_mut().insert_field(
            "__particles",
            Value::from_list(Rc::new(RefCell::new(particles))),
        );
        Ok(())
    }

    /// Per-frame particle aging. Fires `on_update(p, dt)` for each
    /// live particle, advances `age`, computes `age_ratio`, and
    /// drops any particle past its lifetime. When all particles
    /// are dead, marks the emitter despawned. Mirrors
    /// `eval::tick_particle_emitter`.
    fn tick_particle_emitter(
        &mut self,
        emitter: &Rc<RefCell<BcInstance>>,
        dt: f64,
    ) -> Result<(), RuntimeError> {
        let class = emitter.borrow().class.clone();
        let on_update = class.methods.get("on_update").cloned();
        let particles = {
            let __opt = emitter.borrow().get_field("__particles");
            if let Some(__t) = (__opt).as_ref() {
                if __t.is_list() {
                    __t.as_list()
                } else {
                    return Ok(());
                }
            } else {
                return Ok(());
            }
        };
        let snapshot: Vec<Value> = particles.borrow().clone();
        for p in &snapshot {
            if let Some(method) = on_update.clone() {
                self.invoke_method_value(
                    method,
                    Value::from_bc_instance(emitter.clone()),
                    &[*p, Value::from_float(dt)],
                )?;
            }
            if p.is_object() {
                let rc = p.as_object();
                let mut o = rc.borrow_mut();
                let age = {
                    let __opt = o.get_field("age");
                    if let Some(__t) = (__opt).as_ref() {
                        if __t.is_float() {
                            let a = __t.as_float();
                            a + dt
                        } else if __t.is_int_or_boxed_int() {
                            let a = __t.as_int();
                            a as f64 + dt
                        } else {
                            dt
                        }
                    } else {
                        dt
                    }
                };
                let lifetime = {
                    let __opt = o.get_field("lifetime");
                    if let Some(__t) = (__opt).as_ref() {
                        if __t.is_float() {
                            __t.as_float()
                        } else {
                            1.0
                        }
                    } else {
                        1.0
                    }
                };
                o.insert_field("age", Value::from_float(age));
                let ratio = if lifetime > 0.0 {
                    (age / lifetime).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                o.insert_field("age_ratio", Value::from_float(ratio));
            }
        }
        // Drop dead particles.
        particles.borrow_mut().retain(|p| {
            if p.is_object() {
                let rc = p.as_object();
                let age_opt = rc.borrow().get_field("age");
                let lt_opt = rc.borrow().get_field("lifetime");
                if let (Some(age_v), Some(lt_v)) = (age_opt, lt_opt) {
                    if age_v.is_float() && lt_v.is_float() {
                        return age_v.as_float() < lt_v.as_float();
                    }
                }
                true
            } else {
                true
            }
        });
        if particles.borrow().is_empty() {
            emitter.borrow_mut().despawned = true;
        }
        Ok(())
    }

    fn tick_scene(&mut self, scene: &Rc<RefCell<BcInstance>>, dt: f64) -> Result<(), RuntimeError> {
        // Phase 5 fibers + v0.2 sessions 2c + 7: if the fiber
        // is suspended, count down by dt and either keep waiting
        // or resume the entire saved frame stack. Mirrors
        // `eval::tick_scene`'s leading resume-or-skip block.
        let resume_pending = {
            let inst = scene.borrow();
            !inst.fiber_frames.is_empty()
        };
        if resume_pending {
            let new_remaining = scene.borrow().entry_wait_remaining - dt;
            if new_remaining > 0.0 {
                scene.borrow_mut().entry_wait_remaining = new_remaining;
                return Ok(());
            }
            self.resume_state_entry(scene)?;
            if let Some(target) = self.transitioning.take() {
                return self.enter_state(scene, &target);
            }
            // Resuming may have hit another `wait` — the
            // BcInstance now carries fresh fiber_frames. Bail
            // out of the rest of this tick (clocks + on_update
            // stay paused while suspended).
            if !scene.borrow().fiber_frames.is_empty() {
                return Ok(());
            }
        }
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
                Value::from_bc_instance(scene.clone()),
                &[Value::from_float(dt)],
            )?;
            if let Some(target) = self.transitioning.take() {
                return self.enter_state(scene, &target);
            }
        }

        // Phase 5 task 4: predicate hooks. Same edge-triggered
        // semantics as the tree-walker — invoke each predicate
        // chunk, compare against per-instance last value, fire body
        // on false → true. A transition inside a body short-circuits
        // and re-enters immediately.
        let predicates: Vec<(Rc<BcFunction>, Rc<BcFunction>)> = {
            let inst = scene.borrow();
            inst.current_state
                .as_ref()
                .and_then(|n| inst.class.states.get(n))
                .map(|s| s.on_predicates.clone())
                .unwrap_or_default()
        };
        for (idx, (pred_func, body_func)) in predicates.iter().enumerate() {
            let value = self.invoke_method_value(
                pred_func.clone(),
                Value::from_bc_instance(scene.clone()),
                &[],
            )?;
            let now_true = is_truthy(&value);
            let prev = scene
                .borrow()
                .predicate_last_values
                .get(idx)
                .copied()
                .unwrap_or(false);
            if idx < scene.borrow().predicate_last_values.len() {
                scene.borrow_mut().predicate_last_values[idx] = now_true;
            }
            if now_true && !prev {
                self.invoke_method_value(
                    body_func.clone(),
                    Value::from_bc_instance(scene.clone()),
                    &[],
                )?;
                if let Some(target) = self.transitioning.take() {
                    return self.enter_state(scene, &target);
                }
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
                    Value::from_bc_instance(scene.clone()),
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
        let state = match scene.borrow().class.states.get(state_name).cloned() {
            Some(s) => s,
            None => {
                let inst = scene.borrow();
                let names: Vec<&String> = inst.class.states.keys().collect();
                let suggestion = crate::value::did_you_mean(state_name, &names).map(str::to_string);
                return Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: format!("no state named '{state_name}'"),
                    help: Some(match suggestion {
                        Some(s) => format!("did you mean `-> {s}`?"),
                        None => {
                            "transitions must target a `state <name>:` declared in the same scene"
                                .to_string()
                        }
                    }),
                });
            }
        };
        {
            let mut inst = scene.borrow_mut();
            inst.current_state = Some(state.name.clone());
            inst.every_timers = vec![0.0; state.every_clocks.len()];
            inst.every_intervals_secs = state.every_clocks.iter().map(|(s, _)| *s).collect();
            // Phase 5 fibers + v0.2 sessions 2c + 7: clear any
            // stale resume state from the previous state.
            // Entering a state always restarts its on_entry from
            // the top.
            inst.fiber_frames.clear();
            inst.fiber_stack.clear();
            inst.entry_wait_remaining = 0.0;
            // Phase 5 task 4: reset predicate edge state. Initial
            // value is `false` so a predicate already true on the
            // first tick fires immediately (matches the
            // tree-walker's behavior).
            inst.predicate_last_values = vec![false; state.on_predicates.len()];
        }
        // Run on_entry. A transition inside the body cascades. A
        // `wait` inside the body emits OP_WAIT, which collapses
        // every frame from the on_entry frame down — see
        // `OpCode::Wait` for the multi-frame save. `invoke_method_value`
        // returns Nil and we fall through.
        //
        // v0.2 session 7: track the on_entry frame's depth so
        // OP_WAIT knows how much of the call stack to capture.
        // The bottom of the fiber is `self.frames.len()` *before*
        // the invoke pushes the on_entry frame.
        let prev_entry_depth = self.state_entry_frame_depth.replace(self.frames.len());
        let result = self.invoke_method_value(
            state.on_entry.clone(),
            Value::from_bc_instance(scene.clone()),
            &[],
        );
        // Restore the previous tracker. Whether the invoke
        // returned, errored, or suspended (and OP_WAIT cleared
        // it to None), the caller's slot is what should be live
        // afterward.
        self.state_entry_frame_depth = prev_entry_depth;
        result?;
        if let Some(next) = self.transitioning.take() {
            return self.enter_state(scene, &next);
        }
        Ok(())
    }

    /// Resume a fiber from where `OP_WAIT` suspended it. Replays
    /// the entire saved frame stack (state-entry + any function
    /// calls suspended above it) and the saved value-stack slice,
    /// then dispatches until the fiber returns to top or hits
    /// another wait. v0.2 session 7 (was single-frame in 2c).
    fn resume_state_entry(&mut self, scene: &Rc<RefCell<BcInstance>>) -> Result<(), RuntimeError> {
        let (saved_frames, saved_stack) = {
            let mut inst = scene.borrow_mut();
            if inst.fiber_frames.is_empty() {
                return Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: "vm: resume_state_entry called without a saved fiber".to_string(),
                    help: None,
                });
            }
            let frames = std::mem::take(&mut inst.fiber_frames);
            let stack = std::mem::take(&mut inst.fiber_stack);
            inst.entry_wait_remaining = 0.0;
            (frames, stack)
        };

        // Re-push the saved value-stack slice. The slice's
        // bottom is the on_entry frame's slot_base; everything
        // above is locals + temporaries across all suspended
        // frames.
        let new_bottom = self.stack.len();
        self.stack.extend(saved_stack);

        // Re-push each saved frame, recovering the absolute
        // slot_base by adding `slot_base_offset` to the new
        // bottom.
        if self.frames.len() + saved_frames.len() > FRAMES_MAX {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: "stack overflow".to_string(),
                help: Some(format!(
                    "call stack exceeded {FRAMES_MAX} frames during fiber resume"
                )),
            });
        }
        let target_depth = self.frames.len();
        for f in saved_frames {
            self.frames.push(CallFrame {
                function: f.function,
                ip: f.ip,
                slot_base: new_bottom + f.slot_base_offset,
            });
        }

        // Track the bottom-of-fiber depth so a deeper OP_WAIT can
        // suspend correctly. The on_entry frame is at
        // `target_depth` in self.frames after we pushed.
        let prev_entry_depth = self.state_entry_frame_depth.replace(target_depth);
        let result = self.dispatch(target_depth + 1);
        self.state_entry_frame_depth = prev_entry_depth;
        let _ = result?;
        // dispatch() left the result on top; pop it so the stack stays clean.
        self.stack.pop();
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
        let class = {
            let __opt = args.first();
            if let Some(__t) = (__opt).as_ref() {
                if __t.is_bc_class() {
                    let c = __t.as_bc_class();
                    c.clone()
                } else {
                    let other = *(*__t);
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
            } else {
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
                message: format!("entities.{name} expected 1 argument, got {}", args.len()),
                help: None,
            });
        }
        let matches: Vec<Rc<RefCell<BcInstance>>> = self
            .active_entities
            .iter()
            .filter(|e| !e.borrow().despawned && Rc::ptr_eq(&e.borrow().class, &class))
            .cloned()
            .collect();
        match name {
            "count" => Ok(Value::from_int(matches.len() as i64)),
            "of" => {
                let list: Vec<Value> = matches.into_iter().map(Value::from_bc_instance).collect();
                Ok(Value::from_list(Rc::new(RefCell::new(list))))
            }
            _ => Err(RuntimeError {
                line,
                col: 0,
                message: format!("entities has no method '{name}'"),
                help: Some("entities methods are .of(Class), .count(Class)".to_string()),
            }),
        }
    }

    /// Look up a global by name. Returns a clone — for `Object` /
    /// `BcInstance` / `List` values, the inner Rc is shared, so a
    /// caller that calls `.borrow_mut()` on the cloned Value will
    /// mutate the global in place. Used by the macroquad play loop
    /// to push input state into the `key` / `key_press` / `screen`
    /// Objects each frame.
    pub fn get_global(&self, name: &str) -> Option<Value> {
        self.globals.get(name).cloned()
    }

    /// Drain `vm.out` (captured `print` output). Returns the captured
    /// text and resets the buffer to empty. Used by the play loop
    /// to flush per-frame output to the host's stdout.
    pub fn take_out(&mut self) -> String {
        std::mem::take(&mut self.out)
    }

    /// Update `time.dt` on the global `time` Object so scene/entity
    /// code can read it as an ambient. Mirrors `eval::update_time_ambient`.
    fn update_time_dt(&mut self, dt: f64) {
        if let Some(t) = self.globals.get("time") {
            if t.is_object() {
                let rc = t.as_object();
                rc.borrow_mut().insert_field("dt", Value::from_float(dt));
            }
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
        self.push(recv);
        for arg in args {
            self.push(*arg);
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

    /// Push a legacy `Value` onto the value stack. v0.2 Phase 8.5
    /// session 8c: shims through `TaggedValue::from_legacy`. Inner
    /// pattern matches still receive `Value`; the boundary is here.
    #[inline]
    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }

    /// Same as `push` but takes a `TaggedValue` directly. Used by
    /// hot paths that already hold a tagged slot (e.g.
    /// fiber-resume `extend`) so we avoid a redundant
    /// to_legacy → from_legacy round-trip.
    #[allow(dead_code)]
    fn push_tagged(&mut self, v: TaggedValue) {
        self.stack.push(v);
    }

    #[inline]
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

    /// Read a slot at absolute index, returning the legacy `Value`.
    /// v0.2 Phase 8.5 session 8c: replaces inline
    /// `self.stack.get(abs).cloned()` patterns.
    fn slot_get(&self, abs: usize) -> Option<Value> {
        self.stack.get(abs).cloned()
    }

    /// Write a value into a slot at absolute index.
    fn slot_set(&mut self, abs: usize, v: Value) {
        self.stack[abs] = v;
    }

    /// Peek the top of stack without popping. Returns the legacy
    /// `Value` for compatibility with the dispatch loop's
    /// pattern-match handlers.
    fn peek_top(&self) -> Option<Value> {
        self.stack.last().cloned()
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
        let recv_clone = self.stack[recv_idx];
        if recv_clone.is_bc_instance() {
            let inst_rc = recv_clone.as_bc_instance();
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
        if recv_clone.is_object() {
            let rc = recv_clone.as_object();
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
        if recv_clone.is_object() {
            let rc = recv_clone.as_object();
            let field = rc.borrow().get_field(name).ok_or_else(|| RuntimeError {
                line,
                col: 0,
                message: format!("module `{}` has no field '{name}'", rc.borrow().kind),
                help: None,
            })?;
            let args: Vec<Value> = self.stack.drain(recv_idx + 1..).collect();
            self.stack.pop(); // drop the receiver
            let result = if field.is_builtin() {
                let (bname, params, func) = field.as_builtin();
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
            } else {
                let other = field;
                return Err(RuntimeError {
                    line,
                    col: 0,
                    message: format!(
                        "field `.{name}` on module is a {}, not callable",
                        other.type_name()
                    ),
                    help: None,
                });
            };
            self.push(result);
            return Ok(());
        }
        // Built-in receivers: list / range methods (session 10).
        let args: Vec<Value> = self.stack.drain(recv_idx + 1..).collect();
        let recv = self.stack.pop().expect("receiver");
        let result = if recv.is_list() {
            let rc = recv.as_list();
            list_method(&rc, name, &args, line)?
        } else if recv.is_range() {
            let (start, end, exclusive) = recv.as_range();
            range_method(start, end, exclusive, name, &args, line, &mut self.rng)?
        } else {
            return Err(RuntimeError {
                line,
                col: 0,
                message: format!("method `.{name}` is not defined on {}", recv.type_name()),
                help: None,
            });
        };
        self.push(result);
        Ok(())
    }

    #[inline]
    fn binary_arith(&mut self, op: ArithOp, line: u32) -> Result<(), RuntimeError> {
        // Phase 11 session 7: hot-path peeking. The previous
        // shape — `pop, pop, apply_arith, push` — drained the stack
        // and then refilled it on every arithmetic op even for the
        // overwhelming common case of int+int. Reading the top two
        // slots in place and overwriting with `truncate(len-1)` +
        // direct write avoids two `Vec::pop` underflow checks and
        // one `Vec::push` capacity check per op.
        let len = self.stack.len();
        if len < 2 {
            return Err(RuntimeError {
                line,
                col: 0,
                message: "vm: stack underflow on binary op".to_string(),
                help: None,
            });
        }
        let l = self.stack[len - 2];
        let r = self.stack[len - 1];
        // Phase 29 session 3: immediate-int hot path.
        // The previous `is_int_or_boxed_int` predicate compiled to
        // a chain of tag-bit, then OBJ-tag, then HeapBody-kind
        // probes; the `as_int` extractor then re-checked the same
        // predicate before sign-extending. Replacing both with
        // `is_int` + `as_imm_int_unchecked` collapses to one
        // tag-mask compare and a branchless arithmetic shift per
        // operand. Boxed-i64 (numbers outside ±2^47) fall through
        // to `apply_arith`'s slower path — that path stays
        // correct, the win is dropping one boxed branch from
        // every immediate-int dispatch.
        if l.is_int() && r.is_int() {
            let a = l.as_imm_int_unchecked();
            let b = r.as_imm_int_unchecked();
            let v = match op {
                ArithOp::Div if b == 0 => return Err(division_by_zero(line)),
                ArithOp::Add => Value::from_int(a + b),
                ArithOp::Sub => Value::from_int(a - b),
                ArithOp::Mul => Value::from_int(a * b),
                ArithOp::Div => Value::from_int(a / b),
            };
            self.stack.truncate(len - 1);
            self.stack[len - 2] = v;
            return Ok(());
        }
        // Hot fast path for float+float.
        if l.is_float() && r.is_float() {
            let a = l.as_float();
            let b = r.as_float();
            let v = match op {
                ArithOp::Add => Value::from_float(a + b),
                ArithOp::Sub => Value::from_float(a - b),
                ArithOp::Mul => Value::from_float(a * b),
                ArithOp::Div => Value::from_float(a / b),
            };
            self.stack.truncate(len - 1);
            self.stack[len - 2] = v;
            return Ok(());
        }
        // Slow path: strings, tuples, mixed int/float, errors,
        // and the rare boxed-i64 case.
        let result = apply_arith(op, &l, &r, line)?;
        self.stack.truncate(len - 1);
        self.stack[len - 2] = result;
        Ok(())
    }

    fn binary_mod(&mut self, line: u32) -> Result<(), RuntimeError> {
        let r = self.pop()?;
        let l = self.pop()?;
        if !(l.is_int_or_boxed_int() && r.is_int_or_boxed_int()) {
            return Err(type_error("%", &l, &r, line));
        }
        let a = l.as_int();
        let b = r.as_int();
        if b == 0 {
            return Err(division_by_zero(line));
        }
        let result = Value::from_int(a % b);
        self.push(result);
        Ok(())
    }

    fn unary_neg(&mut self, line: u32) -> Result<(), RuntimeError> {
        let v = self.pop()?;
        let result = if v.is_int_or_boxed_int() {
            let n = v.as_int();
            Value::from_int(-n)
        } else if v.is_float() {
            let f = v.as_float();
            Value::from_float(-f)
        } else {
            let other = v;
            return Err(RuntimeError {
                line,
                col: 0,
                message: format!("unary `-` is not defined on {}", other.type_name()),
                help: None,
            });
        };
        self.push(result);
        Ok(())
    }

    #[inline]
    fn compare(
        &mut self,
        op_str: &str,
        line: u32,
        int_cmp: fn(i64, i64) -> bool,
        float_cmp: fn(f64, f64) -> bool,
    ) -> Result<(), RuntimeError> {
        // Phase 11 session 7: same in-place rewrite as binary_arith.
        let len = self.stack.len();
        if len < 2 {
            return Err(RuntimeError {
                line,
                col: 0,
                message: "vm: stack underflow on compare".to_string(),
                help: None,
            });
        }
        let l = self.stack[len - 2];
        let r = self.stack[len - 1];
        // Phase 29 session 3: immediate-int + immediate-int hot path
        // first. Rest of the chain unchanged — boxed-i64 + mixed-type
        // paths fall through to the original `is_int_or_boxed_int`
        // predicate. The fast path handles nearly every loop counter
        // and `for i in 0..N` body in shipped Twe code.
        let result = if l.is_int() && r.is_int() {
            Value::from_bool(int_cmp(l.as_imm_int_unchecked(), r.as_imm_int_unchecked()))
        } else if l.is_float() && r.is_float() {
            Value::from_bool(float_cmp(l.as_float(), r.as_float()))
        } else if l.is_int_or_boxed_int() && r.is_int_or_boxed_int() {
            Value::from_bool(int_cmp(l.as_int(), r.as_int()))
        } else if l.is_int_or_boxed_int() && r.is_float() {
            Value::from_bool(float_cmp(l.as_int() as f64, r.as_float()))
        } else if l.is_float() && r.is_int_or_boxed_int() {
            Value::from_bool(float_cmp(l.as_float(), r.as_int() as f64))
        } else {
            return Err(type_error(op_str, &l, &r, line));
        };
        self.stack.truncate(len - 1);
        self.stack[len - 2] = result;
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
    v.is_truthy()
}

fn values_equal(l: &Value, r: &Value) -> bool {
    l.equals(r)
}

/// Numeric / string / tuple arithmetic. Mirrors `eval::apply_arith`
/// but takes a small `ArithOp` enum so the VM dispatch can stay
/// flat. Tuple element-wise + / - and tuple <-> scalar * / / are
/// here so Snake-style `cell * cell_size` and `pos + direction`
/// produce the same Tuple values as the tree-walker.
///
/// Phase 11 session 7: int+int and float+float fast paths are
/// hoisted to the top so tight numeric loops short-circuit out
/// before testing for strings, tuples, or mixed numerics. The
/// VM-side `binary_arith` handles the same two cases inline so
/// the ideal call frequency for this function approaches zero
/// in numeric-heavy benchmarks; keeping the fast paths here too
/// covers code paths (constant folding, tuple element recursion)
/// that reach `apply_arith` directly.
#[inline]
fn apply_arith(op: ArithOp, l: &Value, r: &Value, line: u32) -> Result<Value, RuntimeError> {
    // Hot path: int + int.
    if l.is_int_or_boxed_int() && r.is_int_or_boxed_int() {
        let a = l.as_int();
        let b = r.as_int();
        return Ok(match op {
            ArithOp::Div if b == 0 => return Err(division_by_zero(line)),
            ArithOp::Add => Value::from_int(a + b),
            ArithOp::Sub => Value::from_int(a - b),
            ArithOp::Mul => Value::from_int(a * b),
            ArithOp::Div => Value::from_int(a / b),
        });
    }
    // Hot path: float + float.
    if l.is_float() && r.is_float() {
        let a = l.as_float();
        let b = r.as_float();
        return Ok(match op {
            ArithOp::Add => Value::from_float(a + b),
            ArithOp::Sub => Value::from_float(a - b),
            ArithOp::Mul => Value::from_float(a * b),
            ArithOp::Div => Value::from_float(a / b),
        });
    }
    // String concatenation via `+`.
    if matches!(op, ArithOp::Add) && l.is_str() && r.is_str() {
        let a = l.as_string();
        let b = r.as_string();
        let mut s = String::with_capacity(a.len() + b.len());
        s.push_str(a.as_str());
        s.push_str(b.as_str());
        return Ok(Value::from_string(s));
    }
    // Tuple element-wise + / -.
    if l.is_tuple() && r.is_tuple() {
        let a = l.as_tuple();
        let b = r.as_tuple();
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
            return Ok(Value::from_tuple(Rc::new(out)));
        }
    }
    // Tuple * / / scalar.
    if l.is_tuple() {
        let elems = l.as_tuple();
        if matches!(op, ArithOp::Mul | ArithOp::Div) && is_scalar(r) {
            let mut out = Vec::with_capacity(elems.len());
            for x in elems.iter() {
                out.push(apply_arith(op, x, r, line)?);
            }
            return Ok(Value::from_tuple(Rc::new(out)));
        }
    }
    // scalar * Tuple.
    if r.is_tuple() {
        let elems = r.as_tuple();
        if matches!(op, ArithOp::Mul) && is_scalar(l) {
            let mut out = Vec::with_capacity(elems.len());
            for y in elems.iter() {
                out.push(apply_arith(op, l, y, line)?);
            }
            return Ok(Value::from_tuple(Rc::new(out)));
        }
    }
    // Mixed int / float — only reachable now that the homogeneous
    // int+int and float+float fast paths short-circuit at the top.
    if l.is_int_or_boxed_int() && r.is_float() {
        return mix_float(op, l.as_int() as f64, r.as_float(), line);
    }
    if l.is_float() && r.is_int_or_boxed_int() {
        return mix_float(op, l.as_float(), r.as_int() as f64, line);
    }
    Err(type_error(op.as_str(), l, r, line))
}

fn mix_float(op: ArithOp, a: f64, b: f64, line: u32) -> Result<Value, RuntimeError> {
    Ok(match op {
        ArithOp::Add => Value::from_float(a + b),
        ArithOp::Sub => Value::from_float(a - b),
        ArithOp::Mul => Value::from_float(a * b),
        ArithOp::Div => {
            if b == 0.0 {
                return Err(division_by_zero(line));
            }
            Value::from_float(a / b)
        }
    })
}

fn is_scalar(v: &Value) -> bool {
    v.is_number()
}

/// Mirrors `eval::index_get`. Lists and tuples are 0-indexed;
/// negative indices count from the end (Principle 3).
fn index_get(obj: &Value, idx: &Value, line: u32) -> Result<Value, RuntimeError> {
    if obj.is_list() {
        if !idx.is_int_or_boxed_int() {
            return Err(RuntimeError {
                line,
                col: 0,
                message: format!("index must be int, got {}", idx.type_name()),
                help: None,
            });
        }
        let rc = obj.as_list();
        let i = idx.as_int();
        let v = rc.borrow();
        let len = v.len() as i64;
        let actual = if i < 0 { i + len } else { i };
        if actual < 0 || actual >= len {
            return Err(RuntimeError {
                line,
                col: 0,
                message: format!("list index {i} out of bounds (length {len})"),
                help: Some("lists are 0-indexed; negative indices count from the end".to_string()),
            });
        }
        Ok(v[actual as usize])
    } else if obj.is_tuple() {
        if !idx.is_int_or_boxed_int() {
            return Err(RuntimeError {
                line,
                col: 0,
                message: format!("index must be int, got {}", idx.type_name()),
                help: None,
            });
        }
        let elems = obj.as_tuple();
        let i = idx.as_int();
        let len = elems.len() as i64;
        let actual = if i < 0 { i + len } else { i };
        if actual < 0 || actual >= len {
            return Err(RuntimeError {
                line,
                col: 0,
                message: format!("tuple index {i} out of bounds (length {len})"),
                help: None,
            });
        }
        Ok(elems[actual as usize])
    } else {
        Err(RuntimeError {
            line,
            col: 0,
            message: format!("cannot index value of type {}", obj.type_name()),
            help: Some("indexing works on lists and tuples".to_string()),
        })
    }
}

/// Mirrors `eval::field_get` for the subset the bytecode VM
/// reaches: tuples, lists, BcInstances, and Objects (the latter
/// covers module builtins like `math`, `time`, `key`).
fn field_get(obj: &Value, name: &str, line: u32) -> Result<Value, RuntimeError> {
    if obj.is_tuple() {
        let elems = obj.as_tuple();
        match name {
            "x" if !elems.is_empty() => Ok(elems[0]),
            "y" if elems.len() >= 2 => Ok(elems[1]),
            "z" if elems.len() >= 3 => Ok(elems[2]),
            _ => Err(RuntimeError {
                line,
                col: 0,
                message: format!("tuple has no field '{name}'"),
                help: Some(
                    "tuples expose .x, .y, .z (and only those for the leading components)"
                        .to_string(),
                ),
            }),
        }
    } else if obj.is_list() {
        let rc = obj.as_list();
        match name {
            "length" => Ok(Value::from_int(rc.borrow().len() as i64)),
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
        }
    } else if obj.is_bc_instance() {
        let rc = obj.as_bc_instance();
        let inst = rc.borrow();
        inst.get_field(name).ok_or_else(|| RuntimeError {
            line,
            col: 0,
            message: format!(
                "field '{name}' is not defined on instance of {}",
                inst.class.name
            ),
            help: None,
        })
    } else if obj.is_object() {
        let rc = obj.as_object();
        let result = {
            let borrowed = rc.borrow();
            borrowed.get_field(name).ok_or_else(|| RuntimeError {
                line,
                col: 0,
                message: format!("module `{}` has no field '{name}'", borrowed.kind),
                help: None,
            })
        };
        result
    } else {
        let other = *obj;
        Err(RuntimeError {
            line,
            col: 0,
            message: format!("cannot read field '.{name}' on a {}", other.type_name()),
            help: None,
        })
    }
}

/// `recv.name = value`. BcInstance stores in its fields HashMap;
/// Object likewise. Other receivers error.
fn field_set(recv: &Value, name: &str, value: Value, line: u32) -> Result<(), RuntimeError> {
    if recv.is_bc_instance() {
        let rc = recv.as_bc_instance();
        rc.borrow_mut().insert_field(name.to_string(), value);
        Ok(())
    } else if recv.is_object() {
        let rc = recv.as_object();
        rc.borrow_mut().insert_field(name.to_string(), value);
        Ok(())
    } else {
        let other = *recv;
        Err(RuntimeError {
            line,
            col: 0,
            message: format!("cannot assign field '.{name}' on a {}", other.type_name()),
            help: None,
        })
    }
}

/// Walk the class's defaults to materialise a fresh instance.
/// Defaults are cloned so each instance gets its own copy
/// (Rc-shared values like Lists still share their interior, which
/// matches the tree-walker semantics). State-machine fields stay
/// empty; `enter_state` populates them when the scene boots.
fn instantiate_bc(class: Rc<BcClassDef>) -> Value {
    let fields: HashMap<String, TaggedValue> = class
        .field_defaults
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    Value::from_bc_instance(Rc::new(RefCell::new(BcInstance {
        class,
        fields,
        current_state: None,
        every_timers: Vec::new(),
        every_intervals_secs: Vec::new(),
        despawned: false,
        death_fired: false,
        fiber_frames: Vec::new(),
        fiber_stack: Vec::new(),
        entry_wait_remaining: 0.0,
        predicate_last_values: Vec::new(),
    })))
}

/// Mirrors `eval::value_in`. List/Tuple/Range/Str membership.
fn value_in(needle: &Value, haystack: &Value, line: u32) -> Result<bool, RuntimeError> {
    if haystack.is_list() {
        let rc = haystack.as_list();
        let answer = rc.borrow().iter().any(|v| values_equal(v, needle));
        Ok(answer)
    } else if haystack.is_tuple() {
        let elems = haystack.as_tuple();
        Ok(elems.iter().any(|v| values_equal(v, needle)))
    } else if haystack.is_range() {
        let (start, end, exclusive) = haystack.as_range();
        if needle.is_int_or_boxed_int() {
            let n = needle.as_int();
            let upper = if exclusive { end } else { end + 1 };
            Ok(n >= start && n < upper)
        } else {
            Ok(false)
        }
    } else if haystack.is_str() {
        let s = haystack.as_string();
        if needle.is_str() {
            let sub = needle.as_string();
            Ok(s.contains(sub.as_str()))
        } else {
            Ok(false)
        }
    } else {
        let other = *haystack;
        Err(RuntimeError {
            line,
            col: 0,
            message: format!(
                "`in` expects a list, tuple, range, or string, got {}",
                other.type_name()
            ),
            help: None,
        })
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
            rc.borrow_mut().push(args[0]);
            Ok(Value::NIL)
        }
        "prepend" => {
            arity_check(1)?;
            rc.borrow_mut().insert(0, args[0]);
            Ok(Value::NIL)
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
            Ok(Value::from_bool(found))
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
            Ok(Value::from_int(start + (n % span) as i64))
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
            let result = {
                let __t = &args[0];
                if __t.is_int_or_boxed_int() {
                    let n = __t.as_int();
                    n >= start && n < upper
                } else {
                    false
                }
            };
            Ok(Value::from_bool(result))
        }
        _ => Err(RuntimeError {
            line,
            col: 0,
            message: format!("range has no method '{name}'"),
            help: Some("range methods are .roll, .contains".to_string()),
        }),
    }
}

/// Build a fresh particle Object with the same default-field set
/// as `eval::make_particle`: pos, velocity, color, size, age,
/// age_ratio, lifetime. Particle bodies (`on_spawn`, `on_update`)
/// receive this Object as `p` and can mutate any field.
fn make_particle(initial_pos: &Value, lifetime: f64) -> Value {
    let mut fields = HashMap::new();
    fields.insert("pos".to_string(), *initial_pos);
    fields.insert(
        "velocity".to_string(),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(0.0),
            Value::from_float(0.0),
        ])),
    );
    fields.insert(
        "color".to_string(),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(1.0),
            Value::from_float(1.0),
            Value::from_float(1.0),
            Value::from_float(1.0),
        ])),
    );
    fields.insert("size".to_string(), Value::from_float(4.0));
    fields.insert("age".to_string(), Value::from_float(0.0));
    fields.insert("age_ratio".to_string(), Value::from_float(0.0));
    fields.insert("lifetime".to_string(), Value::from_float(lifetime));
    Value::from_object(Rc::new(RefCell::new(crate::value::Object {
        fields,
        kind: "particle",
    })))
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
        assert!(run_expr("1 + 2")
            .map(|v| v.is_int_or_boxed_int() && v.as_int() == 3)
            .unwrap_or(false));
        assert!(run_expr("7 - 4")
            .map(|v| v.is_int_or_boxed_int() && v.as_int() == 3)
            .unwrap_or(false));
        assert!(run_expr("3 * 4")
            .map(|v| v.is_int_or_boxed_int() && v.as_int() == 12)
            .unwrap_or(false));
        assert!(run_expr("10 / 3")
            .map(|v| v.is_int_or_boxed_int() && v.as_int() == 3)
            .unwrap_or(false));
        assert!(run_expr("1 + 2 * 3")
            .map(|v| v.is_int_or_boxed_int() && v.as_int() == 7)
            .unwrap_or(false));
    }

    #[test]
    fn vm_evaluates_float_arithmetic_with_int_promotion() {
        assert!(run_expr("1.5 + 0.5")
            .map(|v| v.is_float() && v.as_float() == 2.0)
            .unwrap_or(false));
        assert!(run_expr("1 + 0.5")
            .map(|v| v.is_float() && v.as_float() == 1.5)
            .unwrap_or(false));
        assert!(run_expr("3.0 / 2")
            .map(|v| v.is_float() && v.as_float() == 1.5)
            .unwrap_or(false));
    }

    #[test]
    fn vm_evaluates_unary_neg_and_not() {
        assert!(run_expr("-7")
            .map(|v| v.is_int_or_boxed_int() && v.as_int() == -7)
            .unwrap_or(false));
        assert!(run_expr("not true").map(|v| v.is_falsy()).unwrap_or(false));
        assert!(run_expr("not false")
            .map(|v| v.is_truthy() && v.is_bool() && v.as_bool())
            .unwrap_or(false));
        // Twe truthiness: 0 is truthy; `not 0` is false.
        assert!(run_expr("not 0").map(|v| v.is_falsy()).unwrap_or(false));
    }

    #[test]
    fn vm_evaluates_comparisons() {
        assert!(run_expr("1 < 2")
            .map(|v| v.is_truthy() && v.is_bool() && v.as_bool())
            .unwrap_or(false));
        assert!(run_expr("2 < 1").map(|v| v.is_falsy()).unwrap_or(false));
        assert!(run_expr("3 == 3")
            .map(|v| v.is_truthy() && v.is_bool() && v.as_bool())
            .unwrap_or(false));
        assert!(run_expr("3 != 3").map(|v| v.is_falsy()).unwrap_or(false));
        assert!(run_expr("3 >= 3")
            .map(|v| v.is_truthy() && v.is_bool() && v.as_bool())
            .unwrap_or(false));
        assert!(run_expr("2 <= 1").map(|v| v.is_falsy()).unwrap_or(false));
        // Cross-type numeric: int 3 vs float 3.0 is equal.
        assert!(run_expr("3 == 3.0")
            .map(|v| v.is_truthy() && v.is_bool() && v.as_bool())
            .unwrap_or(false));
    }

    #[test]
    fn vm_concatenates_strings_with_plus() {
        let v = run_expr(r#""hello, " + "world""#).expect("ok");
        if v.is_str() {
            let s = v.as_string();
            assert_eq!(s.as_str(), "hello, world");
        } else {
            let other = v;
            panic!("want Str, got {other:?}");
        }
    }

    #[test]
    fn vm_division_by_zero_errors() {
        let err = run_expr("1 / 0").expect_err("should fail");
        assert!(
            err.message.contains("division by zero"),
            "got: {}",
            err.message
        );
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
        let chunk = crate::compiler::compile_program(&program).expect("compile");
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
        let out = run_program("function greet():\n    print(\"hi\")\n\ngreet()\n").expect("ok");
        assert_eq!(out, "hi\n");
    }

    #[test]
    fn vm_calls_a_function_with_args_and_return() {
        let out =
            run_program("function add(a, b):\n    return a + b\n\nlet r = add(2, 3)\nprint(r)\n")
                .expect("ok");
        assert_eq!(out, "5\n");
    }

    #[test]
    fn vm_function_without_explicit_return_returns_nil() {
        let out = run_program("function noop():\n    let x = 1\n\nlet r = noop()\nprint(r)\n")
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
        assert!(
            err.message.contains("stack overflow"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn vm_arity_mismatch_errors() {
        let src = "function takes_two(a, b):\n    return a + b\n\ntakes_two(1)\n";
        let err = run_program(src).expect_err("should fail");
        assert!(err.message.contains("expected 2"), "got: {}", err.message);
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
            let bytecode_result =
                run_expr(src).unwrap_or_else(|e| panic!("bytecode failed on `{src}`: {e}"));
            // Run the same expression through the tree-walker by
            // wrapping it in `print(...)` and parsing the output.
            let walker_out = crate::eval::run(
                &parser::parse(&lexer::lex(&format!("print({src})\n")).expect("lex"))
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
        let out =
            run_program("let p = (3, 4, 5)\nprint(p[0])\nprint(p[1])\nprint(p[2])").expect("ok");
        assert_eq!(out, "3\n4\n5\n");
    }

    #[test]
    fn vm_tuple_field_xyz() {
        let out =
            run_program("let p = (10, 20, 30)\nprint(p.x)\nprint(p.y)\nprint(p.z)").expect("ok");
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
        assert!(
            err.message.contains("out of bounds"),
            "got: {}",
            err.message
        );
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
        let out = run_program("let total = 0\nfor i in 0..3:\n    total = total + 1\nprint(total)")
            .expect("ok");
        assert_eq!(out, "4\n");
        let out =
            run_program("let total = 0\nfor i in 0..<3:\n    total = total + 1\nprint(total)")
                .expect("ok");
        assert_eq!(out, "3\n");
    }

    #[test]
    fn vm_for_over_range_sums_correctly() {
        // 1+2+...+10 = 55 — classic.
        let out =
            run_program("let sum = 0\nfor i in 1..10:\n    sum = sum + i\nprint(sum)").expect("ok");
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
            let bytecode_out =
                run_program(src).unwrap_or_else(|e| panic!("bytecode failed on `{src}`: {e}"));
            let walker_out =
                crate::eval::run(&parser::parse(&lexer::lex(src).expect("lex")).expect("parse"))
                    .unwrap_or_else(|e| panic!("walker failed on `{src}`: {e}"));
            assert_eq!(bytecode_out, walker_out, "results diverge on `{src}`",);
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
        let out =
            run_program("print(math.min(3, 1))\nprint(math.max(3, 1))\nprint(math.abs(-7))\n")
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
            let bytecode_out =
                run_program(src).unwrap_or_else(|e| panic!("bytecode failed on `{src}`: {e}"));
            let walker_out =
                crate::eval::run(&parser::parse(&lexer::lex(src).expect("lex")).expect("parse"))
                    .unwrap_or_else(|e| panic!("walker failed on `{src}`: {e}"));
            assert_eq!(bytecode_out, walker_out, "results diverge on `{src}`",);
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

    /// Run + return the runtime-error message. Panics if the
    /// program runs to completion (the test wanted an error).
    /// v0.2 session 7.
    fn run_err(src: &str) -> String {
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let chunk = crate::compiler::compile_program(&program).expect("compile");
        let mut vm = VM::new();
        match vm.run(&chunk) {
            Ok(_) => panic!("expected a runtime error, got success"),
            Err(e) => e.message,
        }
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

    // --- Phase 5 fibers in the bytecode VM ---

    #[test]
    fn vm_wait_in_state_suspends_until_duration_elapses() {
        // dt = 0.25, three frames, wait 0.5s → frames 1+2 produce
        // only the on_entry prefix, frame 3 resumes through the
        // transition into `done`. Same shape as the tree-walker
        // test in `tests/eval.rs`.
        let src = concat!(
            "scene Demo:\n",
            "    initial: alert\n",
            "\n",
            "    state alert:\n",
            "        print(\"alert enter\")\n",
            "        wait 0.5s\n",
            "        print(\"alert resume\")\n",
            "        -> done\n",
            "\n",
            "    state done:\n",
            "        print(\"done enter\")\n",
        );
        let out = run_program_frames(src, 3, 0.25).expect("ok");
        assert_eq!(out, "alert enter\nalert resume\ndone enter\n");
    }

    #[test]
    fn vm_wait_resumes_in_one_frame_when_dt_covers_duration() {
        // Single frame, dt = 1.0 covers the 0.5s wait — entry,
        // resume, and the transition all run in the same tick.
        let src = concat!(
            "scene Demo:\n",
            "    initial: alert\n",
            "\n",
            "    state alert:\n",
            "        print(\"alert enter\")\n",
            "        wait 0.5s\n",
            "        print(\"alert resume\")\n",
            "        -> done\n",
            "\n",
            "    state done:\n",
            "        print(\"done enter\")\n",
        );
        let out = run_program_frames(src, 1, 1.0).expect("ok");
        assert_eq!(out, "alert enter\nalert resume\ndone enter\n");
    }

    #[test]
    fn vm_wait_outside_state_body_is_a_runtime_error() {
        // v0.2 session 7: `wait` outside a state on_entry call
        // chain is now a *runtime* error rather than a
        // compile-time one. The compiler drops `allows_wait`
        // because the multi-frame fiber save on `BcInstance`
        // can support function-body wait — provided that
        // function chain is rooted in a state's on_entry
        // (`state_entry_frame_depth.is_some()` at OP_WAIT).
        //
        // Calling `pause()` from top-level (no scene) hits the
        // OP_WAIT runtime check and surfaces the error there.
        let src = "function pause():\n    wait 0.5s\n\npause()\n";
        let err = run_err(src);
        assert!(
            err.contains("active scene") || err.contains("state's on_entry"),
            "expected wait-context runtime error, got: {err}"
        );
    }

    #[test]
    fn vm_wait_inside_function_called_from_state_entry_resumes() {
        // v0.2 session 7: a function called from state on_entry
        // can wait. The fiber stack on `BcInstance` saves both
        // frames; resume replays them in order.
        let src = concat!(
            "function pause_then_log():\n",
            "    print(\"pre-wait\")\n",
            "    wait 0.1s\n",
            "    print(\"post-wait\")\n",
            "\n",
            "scene Demo:\n",
            "    initial: a\n",
            "    state a:\n",
            "        print(\"entry\")\n",
            "        pause_then_log()\n",
            "        print(\"after-call\")\n",
        );
        let out = run_program_frames(src, 2, 0.1).expect("ok");
        assert_eq!(out, "entry\npre-wait\npost-wait\nafter-call\n");
    }

    #[test]
    fn vm_wait_inside_function_inside_if_resumes() {
        // Two-level nesting: function call sits inside an
        // if-then. Two saved frames (state-entry + function),
        // each with its own ip/slot_base.
        let src = concat!(
            "function nap(label):\n",
            "    print(label)\n",
            "    wait 0.1s\n",
            "    print(label)\n",
            "\n",
            "scene Demo:\n",
            "    initial: a\n",
            "    state a:\n",
            "        if true:\n",
            "            print(\"inside-if\")\n",
            "            nap(\"napping\")\n",
            "        print(\"done\")\n",
        );
        let out = run_program_frames(src, 2, 0.1).expect("ok");
        assert_eq!(out, "inside-if\nnapping\nnapping\ndone\n");
    }

    #[test]
    fn vm_two_sequential_waiting_calls_run_in_order() {
        let src = concat!(
            "function step(label):\n",
            "    print(label)\n",
            "    wait 0.1s\n",
            "    print(label)\n",
            "\n",
            "scene Demo:\n",
            "    initial: a\n",
            "    state a:\n",
            "        step(\"first\")\n",
            "        step(\"second\")\n",
            "        print(\"done\")\n",
        );
        let out = run_program_frames(src, 3, 0.1).expect("ok");
        assert_eq!(out, "first\nfirst\nsecond\nsecond\ndone\n");
    }

    #[test]
    fn vm_function_calls_function_with_wait() {
        // Three-frame fiber at suspension time. Resume drains
        // outermost frame's continuation last (state-entry
        // post-`outer()` runs only after the whole chain
        // completes).
        let src = concat!(
            "function inner():\n",
            "    print(\"inner-pre\")\n",
            "    wait 0.1s\n",
            "    print(\"inner-post\")\n",
            "\n",
            "function outer():\n",
            "    print(\"outer-pre\")\n",
            "    inner()\n",
            "    print(\"outer-post\")\n",
            "\n",
            "scene Demo:\n",
            "    initial: a\n",
            "    state a:\n",
            "        outer()\n",
            "        print(\"done\")\n",
        );
        let out = run_program_frames(src, 2, 0.1).expect("ok");
        assert_eq!(out, "outer-pre\ninner-pre\ninner-post\nouter-post\ndone\n");
    }

    // --- Phase 5 task 4: predicate hooks in the bytecode VM ---

    #[test]
    fn vm_predicate_hook_transitions_on_false_to_true() {
        // chase decrements hp via its every-clock; on hp <= 30 the
        // predicate transitions to dead. Edge-triggered: only one
        // "dead" print no matter how many frames after.
        let src = concat!(
            "scene Goblin:\n",
            "    var hp: int = 100\n",
            "    initial: chase\n",
            "    state chase:\n",
            "        every 100ms:\n",
            "            hp -= 25\n",
            "        on hp <= 30:\n",
            "            -> dead\n",
            "    state dead:\n",
            "        print(\"dead\")\n",
        );
        let out = run_program_frames(src, 30, 0.050).expect("ok");
        // 30 frames * 50ms = 1500ms total. With every 100ms, we
        // get 15 fires of `hp -= 25`. Predicate fires when hp
        // first drops to <= 30 (after 3 fires: 75, 50, 25). dead
        // prints exactly once.
        assert!(out.contains("dead"), "expected dead, got: {out:?}");
        assert_eq!(out.matches("dead").count(), 1, "edge-triggered: {out:?}");
    }

    #[test]
    fn vm_predicate_hook_stable_true_fires_once() {
        let src = concat!(
            "scene S:\n",
            "    var fired: int = 0\n",
            "    initial: a\n",
            "    state a:\n",
            "        on true:\n",
            "            fired += 1\n",
            "            print(fired)\n",
        );
        let out = run_program_frames(src, 5, 0.020).expect("ok");
        // True from the start and stays true — body fires exactly
        // once (the false → true edge on the first frame).
        assert_eq!(out, "1\n");
    }

    #[test]
    fn vm_wait_inside_if_in_state_body_resumes() {
        // v0.2 session 2c: `wait` is now permitted inside nested
        // `if` / `while` blocks at the top of state on_entry. The
        // VM's existing single-frame OP_WAIT save handles
        // within-frame suspensions transparently — nested control
        // flow stays in the same VM frame.
        let src = concat!(
            "scene Demo:\n",
            "    initial: a\n",
            "    state a:\n",
            "        if true:\n",
            "            print(\"before\")\n",
            "            wait 0.1s\n",
            "            print(\"after\")\n",
            "        print(\"done\")\n",
        );
        let out = run_program_frames(src, 2, 0.1).expect("ok");
        assert_eq!(out, "before\nafter\ndone\n");
    }

    #[test]
    fn vm_wait_inside_while_in_state_body_resumes() {
        // Three iterations of the inner while, each waiting 0.1s.
        // Each iteration suspends the same VM frame and resumes
        // from the next IP — the loop's back-edge re-evaluates
        // the condition naturally.
        let src = concat!(
            "scene Demo:\n",
            "    initial: a\n",
            "    state a:\n",
            "        var i = 0\n",
            "        while i < 3:\n",
            "            print(\"step\")\n",
            "            wait 0.1s\n",
            "            i = i + 1\n",
            "        print(\"done\")\n",
        );
        let out = run_program_frames(src, 4, 0.1).expect("ok");
        assert_eq!(out, "step\nstep\nstep\ndone\n");
    }

    #[test]
    fn vm_wait_inside_elif_resumes_same_arm() {
        // Pin elif-arm preservation on the VM. The bytecode
        // compiler emits separate jumps per arm; on resume the
        // saved IP lands inside the chosen arm, not back at the
        // elif chain's head.
        let src = concat!(
            "scene Demo:\n",
            "    initial: a\n",
            "    state a:\n",
            "        if false:\n",
            "            print(\"first\")\n",
            "        elif true:\n",
            "            print(\"elif-before\")\n",
            "            wait 0.1s\n",
            "            print(\"elif-after\")\n",
            "        print(\"done\")\n",
        );
        let out = run_program_frames(src, 2, 0.1).expect("ok");
        assert_eq!(out, "elif-before\nelif-after\ndone\n");
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
        let err =
            compile_err("let g = 5\nscene S:\n    var n: int = g\n    initial: a\n    state a:\n");
        assert!(err.contains("literal constant"), "got: {err}");
    }

    #[test]
    fn vm_unknown_initial_state_errors_at_compile() {
        let err = compile_err("scene S:\n    initial: missing\n    state a:\n");
        assert!(err.contains("missing"), "got: {err}");
    }

    // --- Session 13: render + input + particles ---

    /// Helper: run + tick + render in interleaved fashion.
    fn run_and_render(src: &str, frames: u32, dt: f64) -> Result<String, RuntimeError> {
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let chunk = crate::compiler::compile_program(&program).expect("compile");
        let mut vm = VM::new();
        vm.run(&chunk)?;
        for _ in 0..frames {
            vm.tick(dt)?;
            vm.render()?;
        }
        Ok(std::mem::take(&mut vm.out))
    }

    /// Helper: like run_program_frames but lets the test set a key
    /// to pressed before each tick (then clear after).
    fn run_with_key_press(
        src: &str,
        frames: u32,
        dt: f64,
        key: &str,
    ) -> Result<String, RuntimeError> {
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let chunk = crate::compiler::compile_program(&program).expect("compile");
        let mut vm = VM::new();
        vm.run(&chunk)?;
        for _ in 0..frames {
            // Simulate a key being held down each frame; the tree-
            // walker's matching test does the same single-set then
            // ticks repeatedly.
            if let Some(__t) = (vm.get_global("key_press")).as_ref() {
                if __t.is_object() {
                    let rc = __t.as_object();
                    rc.borrow_mut().insert_field(key.to_string(), Value::TRUE);
                }
                vm.tick(dt)?;
            }
        }
        Ok(std::mem::take(&mut vm.out))
    }

    #[test]
    fn vm_on_render_fires_per_render_call() {
        let src = "scene S:\n    var n = 0\n\n    initial: a\n\n    state a:\n        on render():\n            n += 1\n            print(n)\n";
        let out = run_and_render(src, 3, 0.016).expect("ok");
        assert_eq!(out, "1\n2\n3\n");
    }

    #[test]
    fn vm_on_render_does_not_fire_without_render_call() {
        // tick alone shouldn't fire on_render; it only fires when
        // VM::render() is called explicitly.
        let src = "scene S:\n    var n = 0\n\n    initial: a\n\n    state a:\n        on render():\n            n += 1\n            print(n)\n";
        let out = run_program_frames(src, 3, 0.016).expect("ok");
        assert_eq!(out, "");
    }

    #[test]
    fn vm_entity_render_method_fires_on_render() {
        let src = "entity Box:\n    var n = 0\n\n    render():\n        n += 1\n        print(n)\n\nspawn Box at (0, 0)\n";
        let out = run_and_render(src, 2, 0.016).expect("ok");
        assert_eq!(out, "1\n2\n");
    }

    #[test]
    fn vm_on_key_press_dispatches_when_key_held() {
        let src = "scene S:\n    var n = 0\n\n    initial: a\n\n    state a:\n        on key_press.right:\n            n += 1\n            print(n)\n";
        let out = run_with_key_press(src, 3, 0.016, "right").expect("ok");
        assert_eq!(out, "1\n2\n3\n");
    }

    #[test]
    fn vm_on_key_press_no_handler_for_inactive_key() {
        // A scene with handler for `right` only — pressing `left`
        // does nothing. Verifies the per-key dispatch lookup.
        let src = "scene S:\n    var n = 0\n\n    initial: a\n\n    state a:\n        on key_press.right:\n            n += 1\n            print(n)\n";
        let out = run_with_key_press(src, 2, 0.016, "left").expect("ok");
        assert_eq!(out, "");
    }

    #[test]
    fn vm_on_key_press_handler_can_transition() {
        // Mirrors `tests/eval.rs::key_press_handler_fires_when_pressed`.
        let src = "scene S:\n    var counter = 0\n\n    initial: a\n\n    state a:\n        on key_press.right:\n            counter += 1\n            print(counter)\n            if counter >= 2:\n                -> b\n\n    state b:\n        on key_press.right:\n            print(\"done\")\n";
        let out = run_with_key_press(src, 3, 0.016, "right").expect("ok");
        assert_eq!(out, "1\n2\ndone\n");
    }

    #[test]
    fn vm_particles_block_seeds_count_particles_with_defaults() {
        // Mirrors `tests/eval.rs::particles_block_creates_count_particles_with_defaults`.
        let src =
            "particles Spark:\n    count: 4\n    lifetime: 5.0\n\nspawn Spark at (50.0, 60.0)\n";
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let chunk = crate::compiler::compile_program(&program).expect("compile");
        let mut vm = VM::new();
        vm.run(&chunk).expect("run");
        assert_eq!(vm.active_entities.len(), 1);
        let inst = vm.active_entities[0].borrow();
        let n = {
            let __opt = inst.get_field("__particles");
            if let Some(__t) = (__opt).as_ref() {
                if __t.is_list() {
                    let rc = __t.as_list();
                    let l = rc.borrow().len();
                    l
                } else {
                    panic!("__particles should be a list")
                }
            } else {
                panic!("__particles should be a list")
            }
        };
        assert_eq!(n, 4);
    }

    #[test]
    fn vm_particles_on_spawn_runs_per_particle() {
        // count=3 with on_spawn that prints — should fire 3 times
        // at spawn time.
        let src = "particles Burst:\n    count: 3\n    lifetime: 1.0\n\n    on_spawn(p):\n        print(\"seed\")\n\nspawn Burst at (0.0, 0.0)\n";
        let out = run_program_frames(src, 0, 0.0).expect("ok");
        assert_eq!(out, "seed\nseed\nseed\n");
    }

    #[test]
    fn vm_particles_age_and_emitter_despawns() {
        // Mirrors `tests/programs/particles_block.twe` shape.
        // count=3, lifetime=0.1, ticked at 0.05s. After frame 1
        // alive = 1 (one emitter). Frame 2: still alive (particles
        // are at age 0.1 == lifetime, still considered dead by
        // strict <). Actually let me check: tree-walker uses
        // `age < lt` so when age == lt the particle is dead. Frame
        // 1: age=0.05 < 0.1 → alive. Frame 2: age=0.1, NOT < 0.1
        // → dead, all dead, emitter despawns. So count = 1, 1, 0.
        let src = "particles Burst:\n    count: 3\n    lifetime: 0.1\n\n    on_spawn(p):\n        # nothing\n\n    on_update(p, dt):\n        # nothing\n\nspawn Burst at (10.0, 10.0)\n\non update(dt):\n    print(entities.count(Burst))\n";
        let out = run_program_frames(src, 3, 0.05).expect("ok");
        assert_eq!(out, "1\n1\n0\n");
    }

    #[test]
    fn vm_particles_on_update_can_mutate_particle() {
        // on_update(p, dt) sets p.color — the field set should
        // persist across the loop because Object is Rc<RefCell>.
        // Verify by reading the particle's color after one tick.
        let src = "particles Spark:\n    count: 1\n    lifetime: 100.0\n\n    on_update(p, dt):\n        p.size = 99.0\n\nspawn Spark at (0.0, 0.0)\n";
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let chunk = crate::compiler::compile_program(&program).expect("compile");
        let mut vm = VM::new();
        vm.run(&chunk).expect("run");
        vm.tick(0.016).expect("tick");
        let inst = vm.active_entities[0].borrow();
        let particles = {
            let __opt = inst.get_field("__particles");
            if let Some(__t) = (__opt).as_ref() {
                if __t.is_list() {
                    let rc = __t.as_list();
                    let v = rc.borrow().clone();
                    v
                } else {
                    panic!("__particles missing")
                }
            } else {
                panic!("__particles missing")
            }
        };
        assert_eq!(particles.len(), 1);
        let size = {
            let __t = &particles[0];
            if __t.is_object() {
                let rc = __t.as_object();
                let v = rc.borrow().get_field("size");
                v
            } else {
                panic!("particle should be Object")
            }
        };
        assert!(
            size.as_ref()
                .is_some_and(|v| v.is_float() && v.as_float() == 99.0),
            "size = {size:?}"
        );
    }

    #[test]
    fn vm_matches_eval_on_render_input_particles_corpus() {
        // Cross-check render/input/particles programs through both
        // interpreters. Particles use the file under tests/programs
        // so the diff is meaningful.
        let particles_src = std::fs::read_to_string("tests/programs/particles_block.twe")
            .expect("read particles_block.twe");
        // Programs that do tick-only (on_render is exercised in the
        // dedicated test above; cross-checking it would require the
        // tree-walker's render_frame which the eval tests don't drive
        // for arbitrary scenes).
        let cases: &[(&str, u32, f64)] = &[(&particles_src, 3, 0.05)];
        for (src, frames, dt) in cases {
            let bytecode_out = run_program_frames(src, *frames, *dt)
                .unwrap_or_else(|e| panic!("bytecode failed on `{src}`: {e}"));
            let walker_out = crate::eval::run_with_frames(
                &parser::parse(&lexer::lex(src).expect("lex")).expect("parse"),
                *frames,
                *dt,
            )
            .unwrap_or_else(|e| panic!("walker failed on `{src}`: {e}"));
            assert_eq!(bytecode_out, walker_out, "results diverge on `{src}`",);
        }
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
            assert_eq!(bytecode_out, walker_out, "results diverge on `{src}`",);
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
            let bytecode_out =
                run_program(src).unwrap_or_else(|e| panic!("bytecode failed on `{src}`: {e}"));
            let walker_out =
                crate::eval::run(&parser::parse(&lexer::lex(src).expect("lex")).expect("parse"))
                    .unwrap_or_else(|e| panic!("walker failed on `{src}`: {e}"));
            assert_eq!(bytecode_out, walker_out, "results diverge on `{src}`",);
        }
    }
}
