#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

/// A function parameter — name plus an optional type annotation.
/// Non-strict mode parses the annotation but ignores it (the
/// inferer just allocates a fresh var); strict mode unifies the
/// fresh var against the annotation at function-decl time so a
/// call site that disagrees errors. Phase 6 session 2.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Option<crate::types::Type>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        value: Expr,
        ty: Option<crate::types::Type>,
        line: u32,
        col: u32,
    },
    Assign {
        target: AssignTarget,
        op: AssignOp,
        value: Expr,
        line: u32,
        col: u32,
    },
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
    /// Top-level `on render():` handler — fires once per rendered
    /// frame in `twec play3d`. Distinct from
    /// `StateMember::OnRender` (state-scoped, 2D macroquad path);
    /// the top-level form is the 3D-rendering entry point and is
    /// stored on `Env::top_on_render`. Phase 5 task 5 session (d).
    OnRender {
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
    /// `on <ClassName>.<event>(<param>):` — top-level class-event
    /// handler. v0.3 (Phase 9 session 7b) ships only `event = "death"`
    /// (fires when an instance of `<ClassName>` is despawned). The
    /// param binds the dying entity, scoped to the body. Generic
    /// shape so future events (`spawn`, `collide`) can ride the same
    /// AST node.
    OnClassEvent {
        class: String,
        event: String,
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
        params: Vec<Param>,
        ret: Option<crate::types::Type>,
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
    /// `wait <duration>` — cooperative suspension. Inside a state's
    /// on-entry sequence, the runtime stores the next-statement
    /// index and resumes after `duration` elapses. Outside that
    /// context, the tree-walker raises a runtime error (Phase 5
    /// task 2 ships state-entry only; dialogue / `every` rewrite
    /// follow).
    Wait {
        duration: Expr,
        line: u32,
        col: u32,
    },
    /// `dialogue <Name>:` — top-level declaration of a dialogue
    /// routine. The body is a sequence of statements (say, choice,
    /// wait, regular code). Calling `<Name>()` runs the body. Phase
    /// 5 task 3.
    DialogueDecl {
        name: String,
        body: Vec<Stmt>,
        line: u32,
        col: u32,
    },
    /// `say [<actor>:] "<text>"` — dialogue line. With an actor
    /// expression the runtime prints `Actor: text`; without one,
    /// just the text. Phase 5 task 3.
    Say {
        actor: Option<Expr>,
        text: Expr,
        line: u32,
        col: u32,
    },
    /// `choice:` — branching dialogue prompt. Each branch has a
    /// label expression (typically a string literal) and a body.
    /// V0.1 always picks the first branch (deterministic for
    /// testing); real interactive selection ships in a Phase 5
    /// follow-on once the UI surface is designed.
    Choice {
        branches: Vec<(Expr, Vec<Stmt>)>,
        line: u32,
        col: u32,
    },
    /// `import "<path>" [as <alias>]` — module-system import. Phase
    /// 13 session 1 parses the syntax only; session 2 wires the
    /// loader; session 3 wires cross-module name resolution. The
    /// `path` is a forward-slash logical path (e.g. `"math/vec2"`)
    /// which the resolver maps to a real file. The `alias`, when
    /// present, is the name the imported module is bound to in the
    /// current scope; when absent the resolver defaults to the last
    /// path segment.
    Import {
        path: String,
        alias: Option<String>,
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
    /// `visual Name:` — procedural visual block. Phase 9 session 8
    /// adds the keyword + parser + AST node only; the runtime
    /// (WGSL fragment-shader compilation) lands in Phase 9 sessions
    /// 9-11. Bodies are parsed but never type-checked or executed
    /// today, which means scripts can declare them harmlessly.
    Visual,
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
            DeclKind::Visual => "visual",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclMember {
    Field {
        name: String,
        value: Expr,
        /// Optional `: <type>` annotation. Strict mode (Phase 6
        /// session 4) unifies the value's inferred type against
        /// this; non-strict ignores. None when the field was
        /// written without a colon-prefixed type.
        ty: Option<crate::types::Type>,
        line: u32,
        col: u32,
    },
    Method {
        name: String,
        /// Method parameters now carry their annotations the same
        /// way top-level functions do (Phase 6 session 4). Phase 6
        /// session 2 dropped them into `Vec<String>`; this lifts
        /// them back so strict mode can enforce them.
        params: Vec<Param>,
        /// Return type annotation, when present.
        ret: Option<crate::types::Type>,
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
    /// `on <predicate>: body` — predicate event handler. The runtime
    /// evaluates the predicate each frame and fires the body when
    /// the value transitions from false → true (edge-triggered).
    /// Only active while in the enclosing state. Phase 5 task 4.
    OnPredicate {
        predicate: Expr,
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
    Str {
        value: String,
        line: u32,
        col: u32,
    },
    Interp {
        parts: Vec<String>,
        exprs: Vec<Expr>,
        line: u32,
        col: u32,
    },
    Int {
        value: i64,
        line: u32,
        col: u32,
    },
    Float {
        value: f64,
        line: u32,
        col: u32,
    },
    Bool {
        value: bool,
        line: u32,
        col: u32,
    },
    Percent {
        value: f64,
        line: u32,
        col: u32,
    },
    Quantity {
        value: f64,
        unit: String,
        line: u32,
        col: u32,
    },
    Ident {
        name: String,
        line: u32,
        col: u32,
    },
    SelfRef {
        line: u32,
        col: u32,
    },
    Tuple {
        elems: Vec<Expr>,
        line: u32,
        col: u32,
    },
    List {
        elems: Vec<Expr>,
        line: u32,
        col: u32,
    },
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        exclusive: bool,
        line: u32,
        col: u32,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
        line: u32,
        col: u32,
    },
    Field {
        object: Box<Expr>,
        name: String,
        line: u32,
        col: u32,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        kwargs: Vec<(String, Expr)>,
        line: u32,
        col: u32,
    },
    Unary {
        op: UnOp,
        operand: Box<Expr>,
        line: u32,
        col: u32,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        line: u32,
        col: u32,
    },
    /// Single-line `if cond: a elif d: e else: b` ternary form. The
    /// statement-level `if cond: <block>` lives on `Stmt::If`; this
    /// variant is the expression-position counterpart and demands a
    /// mandatory `else` (no nullable expressions).
    IfExpr {
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        elifs: Vec<(Expr, Expr)>,
        else_expr: Box<Expr>,
        line: u32,
        col: u32,
    },
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
            | Expr::Binary { line, .. }
            | Expr::IfExpr { line, .. } => *line,
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
            | Expr::Binary { col, .. }
            | Expr::IfExpr { col, .. } => *col,
        }
    }
}
