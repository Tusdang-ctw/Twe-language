use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

#[derive(Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Percent(f64),
    Quantity { value: f64, unit: Rc<String> },
    Range { start: i64, end: i64, exclusive: bool },
    Str(Rc<String>),
    Tuple(Rc<Vec<Value>>),
    List(Rc<RefCell<Vec<Value>>>),
    Object(Rc<RefCell<Object>>),
    Class(Rc<ClassDef>),
    Instance(Rc<RefCell<Instance>>),
    Function(Rc<FunctionDef>),
    Builtin {
        name: &'static str,
        func: BuiltinFn,
    },
}

#[derive(Debug)]
pub struct FunctionDef {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<crate::ast::Stmt>,
}

#[derive(Debug)]
pub struct ClassDef {
    pub kind: &'static str,
    pub name: String,
    pub parent: Option<Rc<ClassDef>>,
    pub field_defaults: HashMap<String, Value>,
    pub methods: HashMap<String, Rc<MethodDef>>,
    pub states: HashMap<String, Rc<StateDef>>,
    pub initial_state: Option<String>,
}

#[derive(Debug)]
pub struct MethodDef {
    pub params: Vec<String>,
    pub body: Vec<crate::ast::Stmt>,
}

#[derive(Debug)]
pub struct StateDef {
    pub name: String,
    pub on_entry: Vec<crate::ast::Stmt>,
    pub every_clocks: Vec<EveryClockDef>,
    pub on_render: Option<Vec<crate::ast::Stmt>>,
    pub on_key_press: HashMap<String, Vec<crate::ast::Stmt>>,
}

#[derive(Debug)]
pub struct EveryClockDef {
    pub interval: crate::ast::Expr,
    pub body: Vec<crate::ast::Stmt>,
}

#[derive(Debug)]
pub struct Instance {
    pub class: Rc<ClassDef>,
    pub fields: HashMap<String, Value>,
    pub current_state: Option<String>,
    /// Accumulated seconds since each clock last fired, parallel-indexed
    /// to `current_state`'s `every_clocks`.
    pub every_timers: Vec<f64>,
    /// Cached interval seconds for each clock in `current_state`.
    pub every_intervals_secs: Vec<f64>,
    /// Set by `despawn self`; the runtime drops this instance from
    /// `Env::active_entities` at the end of the frame.
    pub despawned: bool,
}

#[derive(Debug, Default)]
pub struct Object {
    pub fields: HashMap<String, Value>,
    pub kind: &'static str,
}

pub type BuiltinFn = fn(&mut Env, &[Value]) -> Result<Value, RuntimeError>;

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "Nil"),
            Value::Bool(b) => write!(f, "Bool({b})"),
            Value::Int(n) => write!(f, "Int({n})"),
            Value::Float(x) => write!(f, "Float({x:?})"),
            Value::Percent(p) => write!(f, "Percent({p})"),
            Value::Quantity { value, unit } => write!(f, "Quantity({value} {unit})"),
            Value::Range { start, end, exclusive } => {
                let op = if *exclusive { "..<" } else { ".." };
                write!(f, "Range({start}{op}{end})")
            }
            Value::Str(s) => write!(f, "Str({s:?})"),
            Value::Tuple(t) => write!(f, "Tuple({t:?})"),
            Value::List(l) => write!(f, "List({:?})", l.borrow()),
            Value::Object(o) => write!(f, "Object({})", o.borrow().kind),
            Value::Class(c) => write!(f, "Class({} {})", c.kind, c.name),
            Value::Instance(i) => write!(f, "Instance({})", i.borrow().class.name),
            Value::Function(func) => write!(f, "Function({})", func.name),
            Value::Builtin { name, .. } => write!(f, "Builtin({name})"),
        }
    }
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Percent(_) => "percent",
            Value::Quantity { .. } => "quantity",
            Value::Range { .. } => "range",
            Value::Str(_) => "string",
            Value::Tuple(_) => "tuple",
            Value::List(_) => "list",
            Value::Object(o) => o.borrow().kind,
            Value::Class(_) => "class",
            Value::Instance(_) => "instance",
            Value::Function(_) => "function",
            Value::Builtin { .. } => "function",
        }
    }

    pub fn display(&self) -> String {
        match self {
            Value::Nil => "nil".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(x) => format!("{x:?}"),
            Value::Percent(p) => format!("{p}%"),
            Value::Quantity { value, unit } => format!("{value}{unit}"),
            Value::Range { start, end, exclusive } => {
                let op = if *exclusive { "..<" } else { ".." };
                format!("{start}{op}{end}")
            }
            Value::Str(s) => s.as_ref().clone(),
            Value::Tuple(elems) => {
                let parts: Vec<String> = elems.iter().map(Value::display).collect();
                format!("({})", parts.join(", "))
            }
            Value::List(rc) => {
                let parts: Vec<String> = rc.borrow().iter().map(Value::display).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Object(o) => format!("<{}>", o.borrow().kind),
            Value::Class(c) => format!("<{} {}>", c.kind, c.name),
            Value::Instance(i) => format!("<{}>", i.borrow().class.name),
            Value::Function(func) => format!("<function {}>", func.name),
            Value::Builtin { name, .. } => format!("<builtin {name}>"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub help: Option<String>,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)?;
        if let Some(help) = &self.help {
            write!(f, "\n  help: {help}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RuntimeError {}

pub struct Env {
    bindings: HashMap<String, Value>,
    pub out: String,
    pub on_update: Option<OnUpdateHandler>,
    pub active_scene: Option<Rc<RefCell<Instance>>>,
    pub active_entities: Vec<Rc<RefCell<Instance>>>,
    pub self_value: Option<Value>,
    pub returning: Option<Value>,
    pub transitioning: Option<String>,
    pub breaking: bool,
    pub continuing: bool,
    pub in_render: bool,
    pub loop_depth: u32,
    pub call_depth: u32,
    rng_state: u64,
}

#[derive(Clone)]
pub struct OnUpdateHandler {
    pub param: String,
    pub body: Vec<crate::ast::Stmt>,
}

impl Env {
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
            out: String::new(),
            on_update: None,
            active_scene: None,
            active_entities: Vec::new(),
            self_value: None,
            returning: None,
            transitioning: None,
            breaking: false,
            continuing: false,
            in_render: false,
            loop_depth: 0,
            call_depth: 0,
            // xorshift64* seeded from a fixed constant for deterministic
            // tests. CLI can override via `twec run --seed N`.
            rng_state: 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// xorshift64* PRNG. Deterministic given a fixed seed.
    pub fn next_random_u64(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn seed_rng(&mut self, seed: u64) {
        // xorshift cannot be seeded with zero; substitute a non-zero value.
        self.rng_state = if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed };
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.bindings.get(name)
    }

    pub fn set(&mut self, name: String, value: Value) {
        self.bindings.insert(name, value);
    }

    pub fn contains(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }

    pub fn remove(&mut self, name: &str) {
        self.bindings.remove(name);
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}
