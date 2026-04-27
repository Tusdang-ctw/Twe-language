#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let { name: String, value: Expr, line: u32, col: u32 },
    Assign { target: AssignTarget, op: AssignOp, value: Expr, line: u32, col: u32 },
    Expr(Expr),
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
    Int { value: i64, line: u32, col: u32 },
    Float { value: f64, line: u32, col: u32 },
    Bool { value: bool, line: u32, col: u32 },
    Ident { name: String, line: u32, col: u32 },
    Tuple { elems: Vec<Expr>, line: u32, col: u32 },
    Field { object: Box<Expr>, name: String, line: u32, col: u32 },
    Call { callee: Box<Expr>, args: Vec<Expr>, line: u32, col: u32 },
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
}

impl Expr {
    pub fn line(&self) -> u32 {
        match self {
            Expr::Str { line, .. }
            | Expr::Int { line, .. }
            | Expr::Float { line, .. }
            | Expr::Bool { line, .. }
            | Expr::Ident { line, .. }
            | Expr::Tuple { line, .. }
            | Expr::Field { line, .. }
            | Expr::Call { line, .. }
            | Expr::Unary { line, .. }
            | Expr::Binary { line, .. } => *line,
        }
    }

    pub fn col(&self) -> u32 {
        match self {
            Expr::Str { col, .. }
            | Expr::Int { col, .. }
            | Expr::Float { col, .. }
            | Expr::Bool { col, .. }
            | Expr::Ident { col, .. }
            | Expr::Tuple { col, .. }
            | Expr::Field { col, .. }
            | Expr::Call { col, .. }
            | Expr::Unary { col, .. }
            | Expr::Binary { col, .. } => *col,
        }
    }
}
