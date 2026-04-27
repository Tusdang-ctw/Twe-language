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
    Str(Rc<String>),
    Tuple(Rc<Vec<Value>>),
    Object(Rc<RefCell<Object>>),
    Builtin {
        name: &'static str,
        func: BuiltinFn,
    },
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
            Value::Str(s) => write!(f, "Str({s:?})"),
            Value::Tuple(t) => write!(f, "Tuple({t:?})"),
            Value::Object(o) => write!(f, "Object({})", o.borrow().kind),
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
            Value::Str(_) => "string",
            Value::Tuple(_) => "tuple",
            Value::Object(o) => o.borrow().kind,
            Value::Builtin { .. } => "function",
        }
    }

    pub fn display(&self) -> String {
        match self {
            Value::Nil => "nil".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(x) => format!("{x:?}"),
            Value::Str(s) => s.as_ref().clone(),
            Value::Tuple(elems) => {
                let parts: Vec<String> = elems.iter().map(Value::display).collect();
                format!("({})", parts.join(", "))
            }
            Value::Object(o) => format!("<{}>", o.borrow().kind),
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
        }
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
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}
