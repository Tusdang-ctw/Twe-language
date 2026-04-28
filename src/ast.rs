#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let { name: String, value: Expr, line: u32, col: u32 },
    Assign { target: AssignTarget, op: AssignOp, value: Expr, line: u32, col: u32 },
    If {
        cond: Expr,
        then_body: Vec<Stmt>,
        elifs: Vec<(Expr, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
        line: u32,
        col: u32,
    },
    OnUpdate {
        param: String,
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
    Decl {
        kind: DeclKind,
        name: String,
        parent: Option<String>,
        members: Vec<DeclMember>,
        line: u32,
        col: u32,
    },
    FunctionDecl {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
    Return {
        value: Option<Expr>,
        line: u32,
        col: u32,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
    For {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
    Break {
        line: u32,
        col: u32,
    },
    Continue {
        line: u32,
        col: u32,
    },
    Transition {
        target: String,
        line: u32,
        col: u32,
    },
    Spawn {
        class: String,
        at: Option<Expr>,
        line: u32,
        col: u32,
    },
    Despawn {
        target: Expr,
        line: u32,
        col: u32,
    },
    Expr(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Entity,
    Item,
    Modifier,
    Inventory,
    Scene,
    Particles,
}

impl DeclKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DeclKind::Entity => "entity",
            DeclKind::Item => "item",
            DeclKind::Modifier => "modifier",
            DeclKind::Inventory => "inventory",
            DeclKind::Scene => "scene",
            DeclKind::Particles => "particles",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclMember {
    Field {
        name: String,
        value: Expr,
        line: u32,
        col: u32,
    },
    Method {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
    InitialState {
        name: String,
        line: u32,
        col: u32,
    },
    State {
        name: String,
        members: Vec<StateMember>,
        line: u32,
        col: u32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum StateMember {
    /// Plain statements that run when the state is entered.
    Stmt(Stmt),
    /// `every <duration>: body` — fires periodically while in the state.
    Every {
        interval: Expr,
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
    /// `on render(): body` — runs each rendered frame while in the state.
    OnRender {
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
    /// `on key_press.<key>: body` — fires once per key down-stroke.
    OnKeyPress {
        key: String,
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
    /// `on update(dt): body` — fires once per frame while in the state.
    /// State-scoped equivalent of the top-level `on update(dt):` handler;
    /// closes Phase 2 frustration F5.
    OnUpdate {
        param: String,
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssignTarget {
    Name(String),
    Field { object: Box<Expr>, name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Set,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Str { value: String, line: u32, col: u32 },
    Interp { parts: Vec<String>, exprs: Vec<Expr>, line: u32, col: u32 },
    Int { value: i64, line: u32, col: u32 },
    Float { value: f64, line: u32, col: u32 },
    Bool { value: bool, line: u32, col: u32 },
    Percent { value: f64, line: u32, col: u32 },
    Quantity { value: f64, unit: String, line: u32, col: u32 },
    Ident { name: String, line: u32, col: u32 },
    SelfRef { line: u32, col: u32 },
    Tuple { elems: Vec<Expr>, line: u32, col: u32 },
    List { elems: Vec<Expr>, line: u32, col: u32 },
    Range { start: Box<Expr>, end: Box<Expr>, exclusive: bool, line: u32, col: u32 },
    Index { object: Box<Expr>, index: Box<Expr>, line: u32, col: u32 },
    Field { object: Box<Expr>, name: String, line: u32, col: u32 },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        kwargs: Vec<(String, Expr)>,
        line: u32,
        col: u32,
    },
    Unary { op: UnOp, operand: Box<Expr>, line: u32, col: u32 },
    Binary { op: BinOp, left: Box<Expr>, right: Box<Expr>, line: u32, col: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Neq,
    Lt,
    Gt,
    Lte,
    Gte,
    And,
    Or,
    In,
    NotIn,
}

impl Expr {
    pub fn line(&self) -> u32 {
        match self {
            Expr::Str { line, .. }
            | Expr::Interp { line, .. }
            | Expr::Int { line, .. }
            | Expr::Float { line, .. }
            | Expr::Bool { line, .. }
            | Expr::Percent { line, .. }
            | Expr::Quantity { line, .. }
            | Expr::Ident { line, .. }
            | Expr::SelfRef { line, .. }
            | Expr::Tuple { line, .. }
            | Expr::List { line, .. }
            | Expr::Range { line, .. }
            | Expr::Index { line, .. }
            | Expr::Field { line, .. }
            | Expr::Call { line, .. }
            | Expr::Unary { line, .. }
            | Expr::Binary { line, .. } => *line,
        }
    }

    pub fn col(&self) -> u32 {
        match self {
            Expr::Str { col, .. }
            | Expr::Interp { col, .. }
            | Expr::Int { col, .. }
            | Expr::Float { col, .. }
            | Expr::Bool { col, .. }
            | Expr::Percent { col, .. }
            | Expr::Quantity { col, .. }
            | Expr::Ident { col, .. }
            | Expr::SelfRef { col, .. }
            | Expr::Tuple { col, .. }
            | Expr::List { col, .. }
            | Expr::Range { col, .. }
            | Expr::Index { col, .. }
            | Expr::Field { col, .. }
            | Expr::Call { col, .. }
            | Expr::Unary { col, .. }
            | Expr::Binary { col, .. } => *col,
        }
    }
}
