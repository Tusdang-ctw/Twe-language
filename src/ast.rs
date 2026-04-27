#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let { name: String, value: Expr, line: u32, col: u32 },
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Str { value: String, line: u32, col: u32 },
    Int { value: i64, line: u32, col: u32 },
    Ident { name: String, line: u32, col: u32 },
    Call { callee: Box<Expr>, args: Vec<Expr>, line: u32, col: u32 },
}

impl Expr {
    pub fn line(&self) -> u32 {
        match self {
            Expr::Str { line, .. }
            | Expr::Int { line, .. }
            | Expr::Ident { line, .. }
            | Expr::Call { line, .. } => *line,
        }
    }

    pub fn col(&self) -> u32 {
        match self {
            Expr::Str { col, .. }
            | Expr::Int { col, .. }
            | Expr::Ident { col, .. }
            | Expr::Call { col, .. } => *col,
        }
    }
}
